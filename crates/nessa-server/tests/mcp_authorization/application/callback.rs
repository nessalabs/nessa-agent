//! Candidate admission through the application owner and loopback boundary.
use super::*;
use crate::mcp_authorization::application::{CallbackBind, ConsentCallback};
use crate::mcp_authorization::infrastructure::LoopbackCallback;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};

fn callback_owner(
    memory: Arc<MemoryAuthorization>,
    callback: Arc<dyn ConsentCallback>,
    http: Arc<dyn OAuthHttp>,
) -> Arc<AuthorizationOwner> {
    Arc::new(AuthorizationOwner::new(
        memory.clone(),
        memory.clone(),
        http,
        callback,
        memory.clone(),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        true,
    ))
}

async fn pending(owner: &Arc<AuthorizationOwner>) -> (String, String, u64) {
    let answer = owner
        .authorize(server(), "docs", "https://mcp.example/mcp")
        .await;
    let AuthorizeAnswer::PendingConsent {
        consent_url,
        deadline_ms,
        ..
    } = answer
    else {
        panic!("expected consent: {answer:?}")
    };
    let url = url::Url::parse(&consent_url).unwrap();
    let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    (
        pairs["state"].clone(),
        pairs["redirect_uri"].clone(),
        deadline_ms,
    )
}

async fn wait_until(mut check: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !check().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn socket_callback(redirect: &str, query: &str) {
    let url = url::Url::parse(redirect).unwrap();
    let host = format!("127.0.0.1:{}", url.port().unwrap());
    let mut stream = TcpStream::connect(host).await.unwrap();
    let request = format!(
        "GET {}?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n",
        url.path()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut page = Vec::new();
    stream.read_to_end(&mut page).await.unwrap();
    assert!(!String::from_utf8_lossy(&page).contains(query));
}

#[tokio::test]
async fn a3c_real_listener_wrong_state_then_valid_closes_before_exchange() {
    let memory = Arc::new(MemoryAuthorization::new());
    push_discovery(&memory).await;
    memory
        .push_route("https://as.example/token", Ok(token_body("access")))
        .await;
    let http = Arc::new(GatedHttp::new(memory.clone()));
    let owner = callback_owner(memory.clone(), Arc::new(LoopbackCallback), http.clone());
    let (state, redirect, _) = pending(&owner).await;
    socket_callback(&redirect, "state=wrong&code=wrong").await;
    assert_eq!(http.entered.load(Ordering::SeqCst), 0);
    let before = memory.load(server()).await.unwrap().unwrap();
    assert!(matches!(before.phase, Phase::PendingConsent { .. }));
    assert!(!before.attempt.unwrap().consumed);
    socket_callback(&redirect, &format!("state={state}&code=valid")).await;
    wait_until(async || http.entered.load(Ordering::SeqCst) == 1).await;
    let url = url::Url::parse(&redirect).unwrap();
    let host = format!("127.0.0.1:{}", url.port().unwrap());
    wait_until(async || TcpStream::connect(&host).await.is_err()).await;
    assert_eq!(http.entered.load(Ordering::SeqCst), 1);
    http.release.store(true, Ordering::SeqCst);
    drop(owner);
    wait_until(async || {
        matches!(
            memory.load(server()).await.unwrap().unwrap().phase,
            Phase::Ready { .. }
        )
    })
    .await;
    assert_eq!(
        memory
            .posts()
            .await
            .iter()
            .filter(|(url, _, _)| url.ends_with("/token"))
            .count(),
        1
    );
}

struct CandidateCallback {
    sender: Mutex<Option<mpsc::Sender<CallbackQuery>>>,
}
impl CandidateCallback {
    fn new() -> Self {
        Self {
            sender: Mutex::new(None),
        }
    }
    async fn sender(&self) -> mpsc::Sender<CallbackQuery> {
        self.sender.lock().await.as_ref().unwrap().clone()
    }
}
#[async_trait]
impl ConsentCallback for CandidateCallback {
    async fn listen(&self, _: Duration) -> Result<CallbackBind, ()> {
        let (sender, candidates) = mpsc::channel(2);
        *self.sender.lock().await = Some(sender);
        Ok(CallbackBind {
            redirect_uri: "http://127.0.0.1:9/mcp-oauth/callback".into(),
            candidates,
        })
    }
}
fn query(state: &str) -> CallbackQuery {
    CallbackQuery {
        state: state.into(),
        code: Some("code".into()),
        denied: false,
    }
}

#[tokio::test]
async fn a3e_preloaded_duplicate_candidates_exchange_once() {
    let memory = Arc::new(MemoryAuthorization::new());
    push_discovery(&memory).await;
    memory
        .push_route("https://as.example/token", Ok(token_body("access")))
        .await;
    let callback = Arc::new(CandidateCallback::new());
    let owner = callback_owner(memory.clone(), callback.clone(), memory.clone());
    let (state, _, _) = pending(&owner).await;
    let sender = callback.sender().await;
    sender.send(query(&state)).await.unwrap();
    let _ = sender.try_send(query(&state));
    tokio::time::timeout(Duration::from_secs(2), sender.closed())
        .await
        .unwrap();
    wait_until(async || owner.facts(server()).await.unwrap().phase == "ready").await;
    assert_eq!(
        memory
            .posts()
            .await
            .iter()
            .filter(|(url, _, _)| url.ends_with("/token"))
            .count(),
        1
    );
}

#[tokio::test]
async fn a3d_denied_expired_or_terminal_stale_closes_candidates() {
    for kind in ["denied", "expired", "terminal-stale"] {
        let memory = Arc::new(MemoryAuthorization::new());
        push_discovery(&memory).await;
        let callback = Arc::new(CandidateCallback::new());
        let owner = callback_owner(memory.clone(), callback.clone(), memory.clone());
        let (state, _, deadline) = pending(&owner).await;
        let sender = callback.sender().await;
        let mut candidate = query(&state);
        match kind {
            "denied" => {
                candidate.code = None;
                candidate.denied = true;
            }
            "expired" => memory.advance_to(deadline + 1).await,
            _ => {
                let denied = CallbackQuery {
                    state,
                    code: None,
                    denied: true,
                };
                assert_eq!(
                    owner.complete_callback(server(), denied).await,
                    AuthorizeAnswer::DiscoveryFailed
                );
                candidate.state = "stale".into();
            }
        }
        sender.send(candidate).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), sender.closed())
            .await
            .unwrap();
        assert!(matches!(
            memory.load(server()).await.unwrap().unwrap().phase,
            Phase::ConsentNeeded
        ));
        assert!(!memory
            .posts()
            .await
            .iter()
            .any(|(url, _, _)| url.ends_with("/token")));
    }
}

#[tokio::test]
async fn a3e_changed_resource_candidate_has_no_exchange_and_terminal_revoke_stops() {
    let memory = Arc::new(MemoryAuthorization::new());
    push_discovery(&memory).await;
    let callback = Arc::new(CandidateCallback::new());
    let owner = callback_owner(memory.clone(), callback.clone(), memory.clone());
    let (state, _, _) = pending(&owner).await;
    let sender = callback.sender().await;
    memory
        .set_resource(server(), "https://replacement.example/mcp")
        .await;
    sender.send(query(&state)).await.unwrap();
    // Revoke overtakes a queued candidate; either queued admission observes
    // the terminal phase or a subsequent candidate does. Neither may exchange.
    let _ = owner.revoke(server()).await;
    let _ = sender.try_send(query(&state));
    tokio::time::timeout(Duration::from_secs(2), sender.closed())
        .await
        .unwrap();
    assert!(!memory
        .posts()
        .await
        .iter()
        .any(|(url, _, _)| url.ends_with("/token")));
}

#[tokio::test]
async fn a2d_exact_issuer_mismatch_refuses_before_registration() {
    let memory = Arc::new(MemoryAuthorization::new());
    memory
        .push_route(
            "https://mcp.example/mcp",
            Ok(OAuthResponse {
                status: 401,
                body: String::new(),
                www_authenticate: Some(
                    "Bearer resource_metadata=\"https://mcp.example/meta\"".into(),
                ),
            }),
        )
        .await;
    memory
        .push_route("https://mcp.example/meta", Ok(metadata()))
        .await;
    let mut response = server_metadata();
    response.body = response.body.replace(
        "\"issuer\":\"https://as.example\"",
        "\"issuer\":\"https://as.example/\"",
    );
    memory
        .push_route(
            "https://as.example/.well-known/oauth-authorization-server",
            Ok(response),
        )
        .await;
    let owner = Arc::new(owner(memory.clone()));
    assert_eq!(
        owner
            .authorize(server(), "docs", "https://mcp.example/mcp")
            .await,
        AuthorizeAnswer::DiscoveryFailed
    );
    assert!(memory.posts().await.is_empty());
}

#[tokio::test]
async fn a3d_accepted_exchange_failures_close_stream_and_keep_typed_phase() {
    for (failure, incomplete) in [
        (OAuthCallFailure::Lost, true),
        (OAuthCallFailure::NotSent, false),
    ] {
        let memory = Arc::new(MemoryAuthorization::new());
        push_discovery(&memory).await;
        memory
            .push_route("https://as.example/token", Err(failure))
            .await;
        let callback = Arc::new(CandidateCallback::new());
        let owner = callback_owner(memory.clone(), callback.clone(), memory.clone());
        let (state, _, _) = pending(&owner).await;
        let sender = callback.sender().await;
        sender.send(query(&state)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), sender.closed())
            .await
            .unwrap();
        drop(owner);
        wait_until(async || {
            let phase = memory.load(server()).await.unwrap().unwrap().phase;
            if incomplete {
                matches!(phase, Phase::AuthorizationIncomplete { .. })
            } else {
                matches!(phase, Phase::ConsentNeeded)
            }
        })
        .await;
        assert_eq!(
            memory
                .posts()
                .await
                .iter()
                .filter(|(url, _, _)| url.ends_with("/token"))
                .count(),
            1
        );
    }
}
