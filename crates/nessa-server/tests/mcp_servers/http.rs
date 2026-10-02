//! The resource route called directly: who may ask, what a ticket buys, and
//! that every refusal is one answer.
use super::*;
use crate::attachments::entrypoint::http as attachments;
use crate::conversation::application::McpAppAuditPhase;
use crate::conversation::application::{ResourceTickets, RESOURCE_TICKET_LIFETIME_MS};
use crate::mcp_servers::domain::ResourceTicketDigest;
use crate::mcp_servers::infrastructure::ticket_test_support::app_initiator;
use crate::mcp_servers::infrastructure::ticket_test_support::{
    app, conversation, held, Fixture, CONVERSATION,
};
use axum::body::to_bytes;

const PAGE: &[u8] = b"<!doctype html><title>chart</title><p>exactly these bytes";

fn route(fixture: &Fixture) -> State<ResourceRoute> {
    State(ResourceRoute::new(Some((
        fixture.store.clone(),
        fixture.audit.clone(),
    ))))
}
fn headers(origin: Option<&str>, tickets: &[&[u8]]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(origin) = origin {
        headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
    }
    for ticket in tickets {
        headers.append(TICKET_HEADER, HeaderValue::from_bytes(ticket).unwrap());
    }
    headers
}
fn issue(fixture: &Fixture) -> String {
    fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap()
}
async fn body(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec()
}

/// A refusal as it reaches the host: status, every header, and the body.
async fn refusal(response: Response) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
    let status = response.status();
    let mut answered: Vec<_> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
        .collect();
    answered.sort();
    (status, answered, body(response).await)
}

#[tokio::test]
async fn issue_then_redeem_serves_exactly_the_held_bytes_with_the_contracts_headers() {
    let fixture = Fixture::new();
    let ticket = issue(&fixture);
    let response = handle_resource(
        route(&fixture),
        headers(Some("tauri://localhost"), &[ticket.as_bytes()]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let answered = response.headers().clone();
    assert_eq!(answered[header::CONTENT_TYPE], "text/html;profile=mcp-app");
    assert_eq!(answered[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(answered[header::CACHE_CONTROL], "no-store");
    assert_eq!(answered[header::CONTENT_DISPOSITION], "attachment");
    assert_eq!(
        answered[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "tauri://localhost"
    );
    assert_eq!(answered[header::VARY], "origin");
    assert_eq!(body(response).await, PAGE);
    // Recorded before it was served, against the call that read it, as the app.
    let records = fixture.audit.take();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].phase,
        McpAppAuditPhase::TicketRedeemed {
            ticket_digest: ResourceTicketDigest::of(&ticket).to_hex()
        }
    );
    assert_eq!(records[0].call_id, "call-call-1");
    assert_eq!(records[0].initiator, app_initiator());
    assert!(fixture.ends.take().is_empty());
}

#[tokio::test]
async fn a_redemption_that_cannot_be_recorded_serves_nothing_and_spends_the_ticket() {
    let fixture = Fixture::new();
    let ticket = issue(&fixture);
    fixture.audit.fail(true);
    let response = handle_resource(
        route(&fixture),
        headers(Some("tauri://localhost"), &[ticket.as_bytes()]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!response.headers().contains_key(header::CONTENT_TYPE));
    assert_eq!(response.headers()[header::VARY], "origin");
    assert!(body(response).await.is_empty());
    // Spent: with the audit back, the ticket buys nothing, and its bytes are
    // let go of.
    fixture.audit.fail(false);
    let again = handle_resource(route(&fixture), headers(None, &[ticket.as_bytes()])).await;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
    assert!(fixture.audit.take().is_empty());
    assert_eq!(fixture.store.held_bytes(&conversation(CONVERSATION)), 0);
    // Nor is it reported as an unredeemed end afterwards.
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    fixture.store.sweep();
    assert!(fixture.ends.take().is_empty());
}

#[tokio::test]
async fn a_second_redemption_of_one_ticket_is_404() {
    let fixture = Fixture::new();
    let ticket = issue(&fixture);
    let first = handle_resource(route(&fixture), headers(None, &[ticket.as_bytes()])).await;
    assert_eq!(first.status(), StatusCode::OK);
    let second = handle_resource(route(&fixture), headers(None, &[ticket.as_bytes()])).await;
    assert_eq!(second.status(), StatusCode::NOT_FOUND);
    assert!(body(second).await.is_empty());
}

#[tokio::test]
async fn every_refused_ticket_is_the_same_empty_404() {
    let fixture = Fixture::new();
    let us = conversation(CONVERSATION);

    let spent = issue(&fixture);
    handle_resource(route(&fixture), headers(None, &[spent.as_bytes()])).await;
    let expired = issue(&fixture);
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    let app_released = issue(&fixture);
    fixture.store.release_app(&us, &app("call-1", "mount-1"));
    let conversation_released = issue(&fixture);
    fixture.store.release_conversation(&us);
    let unknown = crate::mcp_servers::domain::resource_ticket([42; 32]);
    let doubled = issue(&fixture);

    let mut cases: Vec<(&str, Response)> = Vec::new();
    for (name, presented) in [
        ("spent", vec![spent.as_bytes()]),
        ("expired", vec![expired.as_bytes()]),
        ("app released", vec![app_released.as_bytes()]),
        (
            "conversation released",
            vec![conversation_released.as_bytes()],
        ),
        ("never issued", vec![unknown.as_bytes()]),
        ("malformed", vec![b"not a ticket".as_slice()]),
        ("empty", vec![b"".as_slice()]),
        ("not text", vec![b"\xff\xfe".as_slice()]),
        ("missing", vec![]),
        ("doubled", vec![doubled.as_bytes(), doubled.as_bytes()]),
        (
            "one good of two",
            vec![doubled.as_bytes(), unknown.as_bytes()],
        ),
    ] {
        cases.push((
            name,
            handle_resource(route(&fixture), headers(None, &presented)).await,
        ));
    }
    // And a gateway with no MCP server composed, which issued nothing.
    cases.push((
        "no store",
        handle_resource(
            State(ResourceRoute::new(None)),
            headers(None, &[doubled.as_bytes()]),
        )
        .await,
    ));

    let mut answers = Vec::new();
    for (name, response) in cases {
        let answer = refusal(response).await;
        assert_eq!(answer.0, StatusCode::NOT_FOUND, "{name}");
        assert!(answer.2.is_empty(), "{name}");
        answers.push((name, answer));
    }
    for (name, answer) in &answers {
        assert_eq!(
            answer, &answers[0].1,
            "{name} differs from {}",
            answers[0].0
        );
    }
}

#[tokio::test]
async fn a_doubled_or_missing_header_redeems_nothing_and_leaves_the_ticket_good() {
    let fixture = Fixture::new();
    let ticket = issue(&fixture);
    for presented in [vec![], vec![ticket.as_bytes(), ticket.as_bytes()]] {
        let response = handle_resource(route(&fixture), headers(None, &presented)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    assert!(fixture.ends.take().is_empty());
    let response = handle_resource(route(&fixture), headers(None, &[ticket.as_bytes()])).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_head_request_is_refused_and_does_not_spend_the_ticket() {
    let fixture = Fixture::new();
    let ticket = issue(&fixture);
    let response = handle_head(headers(None, &[ticket.as_bytes()])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        handle_head(headers(Some("https://evil.example"), &[]))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = handle_resource(route(&fixture), headers(None, &[ticket.as_bytes()])).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn origins_and_preflight_are_answered_as_at_attachments() {
    for origin in [
        Some("tauri://localhost"),
        Some("http://127.0.0.1:5173"),
        Some("https://evil.example"),
        Some("null"),
        Some("http://127.0.0.1:5173/x"),
        None,
    ] {
        let ours = refusal(handle_preflight(headers(origin, &[])).await).await;
        let theirs = refusal(attachments::handle_preflight(headers(origin, &[])).await).await;
        // Everything alike but what each route lets a page send.
        let differing = |answer: &(StatusCode, Vec<(String, String)>, Vec<u8>)| {
            answer
                .1
                .iter()
                .filter(|(name, _)| {
                    name != "access-control-allow-methods" && name != "access-control-allow-headers"
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(ours.0, theirs.0, "{origin:?}");
        assert_eq!(differing(&ours), differing(&theirs), "{origin:?}");
        if ours.0 == StatusCode::NO_CONTENT {
            let allowed: Vec<_> = ours
                .1
                .iter()
                .filter(|(name, _)| {
                    name.starts_with("access-control-allow-m")
                        || name == "access-control-allow-headers"
                })
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            assert_eq!(
                allowed,
                [
                    ("access-control-allow-headers", "x-nessa-resource-ticket"),
                    ("access-control-allow-methods", "GET"),
                ]
            );
        }
    }
}

#[tokio::test]
async fn a_page_this_server_does_not_trust_cannot_spend_a_ticket() {
    let fixture = Fixture::new();
    let ticket = issue(&fixture);
    let response = handle_resource(
        route(&fixture),
        headers(Some("https://evil.example"), &[ticket.as_bytes()]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!response
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    assert_eq!(response.headers()[header::VARY], "origin");
    // Refused before the ticket was looked at, so it still works.
    let response = handle_resource(
        route(&fixture),
        headers(Some("http://127.0.0.1:5173"), &[ticket.as_bytes()]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "http://127.0.0.1:5173"
    );
}
