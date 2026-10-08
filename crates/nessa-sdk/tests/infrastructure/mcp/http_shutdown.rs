//! ADR 392 J19: a duplicate close must fence effects before returning.
use super::{HttpSession, SendOutcome, SessionBinding, SessionClaims};
use crate::infrastructure::mcp::{
    HttpBody, HttpExchange, HttpFailure, HttpMethod, HttpRequest, HttpResponse, McpError,
    NoAuthorization, RemoteMcpUrl,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{atomic::Ordering, Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

#[derive(Default)]
struct Peer {
    methods: Mutex<Vec<HttpMethod>>,
}
#[async_trait]
impl HttpExchange for Peer {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        self.methods.lock().unwrap().push(request.method);
        Ok(HttpResponse {
            status: 202,
            headers: vec![],
            body: HttpBody::Buffered(vec![]),
        })
    }
}

fn session(peer: Arc<Peer>) -> Arc<HttpSession> {
    let (session, _incoming) = HttpSession::open(
        Uuid::new_v4(),
        RemoteMcpUrl::parse("http://127.0.0.1/mcp").unwrap(),
        peer,
        Arc::new(NoAuthorization),
        Arc::new(SessionClaims::default()),
    );
    session
}

#[tokio::test]
async fn j19_duplicate_shutdown_fences_effects_after_cleanup_claim() {
    let peer = Arc::new(Peer::default());
    let session = session(peer.clone());
    // The first caller has claimed cleanup but is paused before the readers
    // fence: the interleaving permitted by the old shutdown ordering.
    session.delete_started.store(true, Ordering::SeqCst);
    session.shutdown();
    let initialized = json!({
        "protocolVersion": "2025-03-26",
        "capabilities": {},
        "serverInfo": {"name": "peer", "version": "1"},
    });
    let publication = session.publish_initialize(&initialized, Some("late"));
    assert!(matches!(publication, Err(McpError::Closed)));
    assert!(!session.phase.lock().unwrap().open);
    assert!(session.phase.lock().unwrap().binding.is_none());
    assert!(session.readers.lock().unwrap().get.is_empty());
    assert!(session
        .claims
        .claim(session.url.as_str(), "late", session.local + 1));
    let outcome = session
        .dispatch(br#"{"jsonrpc":"2.0","method":"notifications/ping"}"#)
        .await;
    assert!(matches!(outcome, SendOutcome::End(McpError::Closed)));
    assert!(peer.methods.lock().unwrap().is_empty());
}

#[tokio::test]
async fn repeated_shutdown_keeps_one_delete_and_releases_its_claim() {
    let peer = Arc::new(Peer::default());
    let session = session(peer.clone());
    let id = "claimed";
    assert!(session
        .claims
        .claim(session.url.as_str(), id, session.local));
    session.phase.lock().unwrap().binding = Some(Arc::new(SessionBinding {
        id: Some(id.into()),
    }));
    let mut finished = session.finished();
    session.shutdown();
    session.shutdown();
    tokio::time::timeout(Duration::from_secs(2), finished.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(*finished.borrow());
    session.shutdown();
    assert_eq!(*peer.methods.lock().unwrap(), vec![HttpMethod::Delete]);
    assert!(session
        .claims
        .claim(session.url.as_str(), id, session.local + 1));
}
