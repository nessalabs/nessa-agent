//! Who may read the answer, and the names it is given in. The host is a stub:
//! this test must never ask the machine it runs on about credentials.

use super::*;
use crate::agents::application::{AgentProbe, ProbeFailure};
use crate::agents_test_support::{StubAgentProbe, WaitingAgentProbe};
use axum::body::to_bytes;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Long enough that no test here ever reaches it by accident, short enough that
/// a test which does reach it fails rather than stalls.
const TEST_DEADLINE: Duration = Duration::from_secs(5);

fn from(origin: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(value) = origin {
        headers.insert(header::ORIGIN, HeaderValue::from_str(value).unwrap());
    }
    headers
}

/// The route's view of a machine: one shared reader over the given probe.
fn reading(probe: Arc<dyn AgentProbe>, deadline: Duration) -> State<Arc<SharedAgentReadiness>> {
    State(Arc::new(SharedAgentReadiness::with_deadline(
        probe, deadline,
    )))
}

fn host(probe: StubAgentProbe) -> State<Arc<SharedAgentReadiness>> {
    reading(Arc::new(probe), TEST_DEADLINE)
}

/// A host that counts how many times it was asked anything.
#[derive(Default)]
struct CountingProbe {
    asked: AtomicUsize,
}

impl AgentProbe for CountingProbe {
    fn configured(&self, _agent: AgentId) -> bool {
        true
    }

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
    fn configured(&self, _agent: AgentId) -> bool {
        true
    }

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
    let machine = reading(counted.clone(), TEST_DEADLINE);
    let response = handle_http_agents(machine.clone(), from(Some("https://evil.example"))).await;
    assert_eq!(response.status(), 403);
    assert_eq!(counted.asked.load(Ordering::SeqCst), 0);

    let allowed = handle_http_agents(machine, from(None)).await;
    assert_eq!(allowed.status(), 200);
    assert!(counted.asked.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn a_host_that_falls_over_while_being_asked_does_not_take_the_handler_with_it() {
    // The asking happens on a blocking thread, so its failure arrives as a
    // failed join rather than as an unwind through the request.
    let response =
        handle_http_agents(reading(Arc::new(PanickingProbe), TEST_DEADLINE), from(None)).await;
    assert_eq!(response.status(), 500);
}

#[tokio::test]
async fn a_machine_that_will_not_answer_is_reported_as_no_answer_rather_than_as_no_agent() {
    // Gate 7. The three names on the wire are all claims about the agent, and
    // this server found none of them out — it stopped waiting. Saying
    // "not-installed" would send the person to install what they have, and
    // "needs-authentication" would send them to sign in again; both would be
    // this server making something up about their machine.
    let probe = Arc::new(WaitingAgentProbe::default());
    let response = handle_http_agents(
        reading(probe.clone(), Duration::from_millis(50)),
        from(Some("tauri://localhost")),
    )
    .await;
    assert_eq!(response.status(), 503);
    // The page is told it is a refusal to answer rather than left unable to
    // read the status at all.
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some("tauri://localhost")
    );
    probe.release();
}

#[tokio::test]
async fn many_callers_at_once_still_only_ask_this_machine_once() {
    // The route is unauthenticated, so how many requests arrive is not this
    // server's choice. How many probes they turn into is: every caller is asking
    // the same parameterless question, so one probe answers all of them — one
    // blocking thread, and on macOS one `security` process, not eight.
    const CALLERS: usize = 8;
    let probe = Arc::new(WaitingAgentProbe::default());
    let machine = reading(probe.clone(), TEST_DEADLINE);

    let callers: Vec<_> = (0..CALLERS)
        .map(|_| {
            let machine = machine.clone();
            tokio::spawn(async move { handle_http_agents(machine, from(None)).await })
        })
        .collect();
    probe.started().await;
    probe.release();

    for caller in callers {
        assert_eq!(caller.await.unwrap().status(), 200, "every caller answered");
    }
    assert_eq!(
        probe.most_at_once(),
        1,
        "{CALLERS} callers must never put more than one probe on this machine at a time"
    );
    assert!(
        probe.runs() <= CALLERS,
        "a caller arriving after the shared probe finished asks again, but none asks twice"
    );
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

/// Every readiness this route can name, as a list the compiler keeps complete.
///
/// The match is the point of it: a new [`Readiness`] state fails to compile here
/// rather than quietly escaping the seam guard below, which is exactly what
/// happened when the list of names was written out by hand.
const EVERY_READINESS: [Readiness; 5] = [
    Readiness::Ready,
    Readiness::NeedsAuthentication,
    Readiness::AuthenticationUnknown,
    Readiness::NotInstalled,
    Readiness::NotConfigured,
];
fn _every_readiness_is_listed(readiness: Readiness) -> usize {
    match readiness {
        Readiness::Ready => 0,
        Readiness::NeedsAuthentication => 1,
        Readiness::AuthenticationUnknown => 2,
        Readiness::NotInstalled => 3,
        Readiness::NotConfigured => 4,
    }
}

/// The other side of this route is `src/onboarding/adapters/agents.ts`, which
/// hardcodes the same path and every readiness name this route can send, because
/// they cross a language boundary nothing else checks. This is the same shape of
/// guard as `host.rs`'s seam test: a rename on either side fails here instead
/// of at runtime.
#[test]
fn the_frontend_adapter_reads_the_same_path_and_names() {
    let adapter = include_str!("../../../../src/onboarding/adapters/agents.ts");
    assert!(
        adapter.contains("/onboarding/agents"),
        "src/onboarding/adapters/agents.ts does not request /onboarding/agents"
    );
    for readiness in EVERY_READINESS {
        let name = readiness_name(readiness);
        assert!(
            adapter.contains(&format!("\"{name}\"")),
            "src/onboarding/adapters/agents.ts does not recognize {name:?}"
        );
    }
}
