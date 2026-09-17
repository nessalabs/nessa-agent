//! Who may read the answer, and the three names it is given in. The host is a
//! stub: this test must never ask the machine it runs on about credentials.

use super::*;
use crate::agents::application::ProbeFailure;
use crate::agents_test_support::StubAgentProbe;
use axum::body::to_bytes;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

fn from(origin: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(value) = origin {
        headers.insert(header::ORIGIN, HeaderValue::from_str(value).unwrap());
    }
    headers
}

fn host(probe: StubAgentProbe) -> State<Arc<dyn AgentProbe>> {
    State(Arc::new(probe))
}

/// A host that counts how many times it was asked anything.
#[derive(Default)]
struct CountingProbe {
    asked: AtomicUsize,
}

impl AgentProbe for CountingProbe {
    fn installed(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }

    fn authenticated(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
}

/// A host that fails in the one way a blocking thread reports back as a lost task.
struct PanickingProbe;

impl AgentProbe for PanickingProbe {
    fn installed(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        panic!("this machine came apart while being asked");
    }

    fn authenticated(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        Ok(true)
    }
}

async fn readiness_of(probe: StubAgentProbe) -> String {
    let response = handle_http_agents(host(probe), from(None)).await;
    assert_eq!(response.status(), 200);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["agents"][0]["id"], "claude");
    value["agents"][0]["readiness"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn names_are_what_the_wire_and_the_interface_use() {
    assert_eq!(
        readiness_of(StubAgentProbe::answering(true, true)).await,
        "ready"
    );
    assert_eq!(
        readiness_of(StubAgentProbe::answering(true, false)).await,
        "needs-authentication"
    );
    assert_eq!(
        readiness_of(StubAgentProbe::answering(false, false)).await,
        "not-installed"
    );
}

#[tokio::test]
async fn a_sign_in_the_machine_would_not_confirm_is_offered_as_one_to_make() {
    // The wire has three names. Asking is better advice than saying nothing,
    // and it is the action that settles the question either way.
    let readiness = readiness_of(StubAgentProbe {
        installed: Ok(true),
        authenticated: Err(ProbeFailure::Unanswered),
    })
    .await;
    assert_eq!(readiness, "needs-authentication");
}

#[tokio::test]
async fn lets_the_app_read_the_answer() {
    // The app's webview is on tauri://localhost, so every request it makes
    // is cross-origin and needs the header to be released to the page.
    let response = handle_http_agents(
        host(StubAgentProbe::answering(true, true)),
        from(Some("tauri://localhost")),
    )
    .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some("tauri://localhost")
    );
}

#[tokio::test]
async fn refuses_a_page_on_any_other_origin() {
    // Unauthenticated is not the same as open to every site you visit.
    let response = handle_http_agents(
        host(StubAgentProbe::answering(true, true)),
        from(Some("https://evil.example")),
    )
    .await;
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn a_refused_page_never_gets_this_machine_searched_on_its_behalf() {
    // Answering means filesystem reads and, on macOS, a subprocess. A site the
    // server will not answer must not be able to make it do that work at all.
    let counted = Arc::new(CountingProbe::default());
    let response = handle_http_agents(
        State(counted.clone() as Arc<dyn AgentProbe>),
        from(Some("https://evil.example")),
    )
    .await;
    assert_eq!(response.status(), 403);
    assert_eq!(counted.asked.load(Ordering::SeqCst), 0);

    let allowed =
        handle_http_agents(State(counted.clone() as Arc<dyn AgentProbe>), from(None)).await;
    assert_eq!(allowed.status(), 200);
    assert!(counted.asked.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn a_host_that_falls_over_while_being_asked_does_not_take_the_handler_with_it() {
    // The asking happens on a blocking thread, so its failure arrives as a
    // failed join rather than as an unwind through the request.
    let response = handle_http_agents(
        State(Arc::new(PanickingProbe) as Arc<dyn AgentProbe>),
        from(None),
    )
    .await;
    assert_eq!(response.status(), 500);
}

#[tokio::test]
async fn answers_a_caller_that_is_not_a_page_at_all() {
    let response =
        handle_http_agents(host(StubAgentProbe::answering(true, true)), from(None)).await;
    assert_eq!(response.status(), 200);
    assert!(!response
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
}

/// The other side of this route is `src/onboarding/adapters/agents.ts`, which
/// hardcodes the same path and the same three readiness names because they
/// cross a language boundary nothing else checks. This is the same shape of
/// guard as `host.rs`'s seam test: a rename on either side fails here instead
/// of at runtime.
#[test]
fn the_frontend_adapter_reads_the_same_path_and_names() {
    let adapter = include_str!("../../../../src/onboarding/adapters/agents.ts");
    assert!(
        adapter.contains("/onboarding/agents"),
        "src/onboarding/adapters/agents.ts does not request /onboarding/agents"
    );
    for name in ["ready", "needs-authentication", "not-installed"] {
        assert!(
            adapter.contains(&format!("\"{name}\"")),
            "src/onboarding/adapters/agents.ts does not recognize {name:?}"
        );
    }
}
