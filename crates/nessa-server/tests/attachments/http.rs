//! The upload route called directly: who may ask, what a ticket buys, and the
//! exact words of every answer.
use super::*;
use crate::attachments::application::{
    AttachmentLimits, NormalizeError, ReleaseCause, ReleaseRequest,
};
use crate::attachments_test_support::{
    conversation, digest_of, organization, principal, Fixture, StubNormalizer, CONVERSATION,
    OTHER_CONVERSATION,
};
use axum::body::to_bytes;
use futures_util::stream;
use serde_json::Value;
use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

const PDF: &str = "application/pdf";
const BYTES: &[u8] = b"twenty bytes of file";

fn route(fixture: &Fixture) -> State<UploadRoute> {
    State(UploadRoute::new(Some(fixture.service.clone())))
}
fn headers(origin: Option<&str>, tickets: &[&str]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(origin) = origin {
        headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
    }
    for ticket in tickets {
        headers.append(TICKET_HEADER, HeaderValue::from_str(ticket).unwrap());
    }
    headers
}
async fn answer(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}
fn fixture() -> Fixture {
    Fixture::new(AttachmentLimits::default())
}

#[tokio::test]
async fn preflight_lets_the_shell_and_the_dev_server_send_a_ticket_and_nobody_else() {
    for origin in ["tauri://localhost", "http://127.0.0.1:5173"] {
        let response = handle_preflight(headers(Some(origin), &[])).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let answered = response.headers();
        // The origin itself, never a wildcard.
        assert_eq!(answered[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
        assert_eq!(answered[header::ACCESS_CONTROL_ALLOW_METHODS], "PUT");
        assert_eq!(
            answered[header::ACCESS_CONTROL_ALLOW_HEADERS],
            "content-type, x-nessa-upload-ticket"
        );
        assert_eq!(answered[header::ACCESS_CONTROL_MAX_AGE], "600");
        assert_eq!(answered[header::VARY], "origin");
    }
    for origin in ["https://evil.example", "null", "http://127.0.0.1:5173/x"] {
        let response = handle_preflight(headers(Some(origin), &[])).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
        // A refusal of an origin varies by origin like every other answer.
        assert_eq!(response.headers()[header::VARY], "origin");
    }
    // Not a page: nothing to allow, nothing to refuse.
    let response = handle_preflight(headers(None, &[])).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(!response
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
}

#[tokio::test]
async fn an_upload_answers_with_the_reference_a_message_must_use() {
    // Not an image: kept as sent.
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let mut request = headers(Some("tauri://localhost"), &[&ticket]);
    request.insert(header::CONTENT_LENGTH, HeaderValue::from(BYTES.len()));
    let response = handle_upload(route(&fixture), request, Body::from(BYTES)).await;
    assert_eq!(
        response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "tauri://localhost"
    );
    assert_eq!(response.headers()[header::VARY], "origin");
    assert_eq!(
        answer(response).await,
        (
            StatusCode::OK,
            serde_json::json!({
                "digest": digest_of(BYTES).to_string(),
                "mimeType": PDF,
                "size": 20,
            })
        )
    );

    // An image: the answer is the normalized image, not what was sent.
    let fixture = Fixture::with_normalizer(
        AttachmentLimits::default(),
        StubNormalizer::producing(b"small jpeg", "image/jpeg"),
    );
    let ticket = fixture.ticket(CONVERSATION, BYTES, "image/png").await;
    let response = handle_upload(
        route(&fixture),
        headers(None, &[&ticket]),
        Body::from(BYTES),
    )
    .await;
    assert!(!response
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    assert_eq!(
        answer(response).await,
        (
            StatusCode::OK,
            serde_json::json!({
                "digest": digest_of(b"small jpeg").to_string(),
                "mimeType": "image/jpeg",
                "size": 10,
            })
        )
    );
}

#[tokio::test]
async fn a_page_this_server_does_not_trust_cannot_spend_a_ticket() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let response = handle_upload(
        route(&fixture),
        headers(Some("https://evil.example"), &[&ticket]),
        Body::from(BYTES),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!response
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    assert_eq!(response.headers()[header::VARY], "origin");
    // Refused before the ticket was looked at, so it still works.
    let response = handle_upload(
        route(&fixture),
        headers(None, &[&ticket]),
        Body::from(BYTES),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_missing_malformed_repeated_or_spent_ticket_is_one_answer() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let unknown = "0".repeat(64);
    for tickets in [
        vec![],
        vec![""],
        vec!["not-hex"],
        vec![unknown.as_str()],
        // Two tickets is not a choice the route makes, even if one is good.
        vec![ticket.as_str(), unknown.as_str()],
    ] {
        let response = handle_upload(
            route(&fixture),
            headers(Some("tauri://localhost"), &tickets),
            Body::from(BYTES),
        )
        .await;
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "tauri://localhost"
        );
        assert_eq!(response.headers()[header::VARY], "origin");
        assert_eq!(
            answer(response).await,
            (
                StatusCode::UNAUTHORIZED,
                serde_json::json!({"code": "ticket_invalid"})
            ),
            "{tickets:?}"
        );
    }
    let mut non_text = headers(None, &[]);
    non_text.insert(TICKET_HEADER, HeaderValue::from_bytes(&[0xff; 64]).unwrap());
    let response = handle_upload(route(&fixture), non_text, Body::from(BYTES)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // None of that spent the real ticket. Using it does.
    for expected in [StatusCode::OK, StatusCode::UNAUTHORIZED] {
        let response = handle_upload(
            route(&fixture),
            headers(None, &[&ticket]),
            Body::from(BYTES),
        )
        .await;
        assert_eq!(response.status(), expected);
    }
}

#[tokio::test]
async fn a_body_longer_than_its_ticket_is_cut_off_not_collected() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let pulled = Arc::new(AtomicUsize::new(0));
    let counter = pulled.clone();
    // A gigabyte on offer, in 64 KiB chunks, with no Content-Length to warn anyone.
    let chunks = stream::iter(0..16_384).map(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok::<_, Infallible>(vec![0_u8; 65_536])
    });
    let response = handle_upload(
        route(&fixture),
        headers(None, &[&ticket]),
        Body::from_stream(chunks),
    )
    .await;
    assert_eq!(
        answer(response).await,
        (
            StatusCode::BAD_REQUEST,
            serde_json::json!({"code": "size_mismatch"})
        )
    );
    assert_eq!(pulled.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.store.written.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn every_refusal_has_its_status_and_code() {
    struct Case {
        media_type: &'static str,
        sent: &'static [u8],
        declared: Option<usize>,
        arrange: fn(&Fixture),
        status: StatusCode,
        body: Value,
    }
    let case = |sent: &'static [u8], status: StatusCode, body: Value| Case {
        media_type: PDF,
        sent,
        declared: None,
        arrange: |_| {},
        status,
        body,
    };
    let refuse_audit: fn(&Fixture) = |fixture| fixture.audit.refusing.store(true, Ordering::SeqCst);
    let cases = [
        case(
            b"too short",
            StatusCode::BAD_REQUEST,
            serde_json::json!({"code": "size_mismatch"}),
        ),
        Case {
            declared: Some(19),
            ..case(
                BYTES,
                StatusCode::BAD_REQUEST,
                serde_json::json!({"code": "size_mismatch"}),
            )
        },
        case(
            b"twenty bytes of FILE",
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({"code": "digest_mismatch"}),
        ),
        Case {
            arrange: |fixture| fixture.store.keep_fails.store(true, Ordering::SeqCst),
            ..case(
                BYTES,
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({"code": "storage_unavailable"}),
            )
        },
        Case {
            arrange: refuse_audit,
            ..case(
                BYTES,
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({"code": "audit_unavailable"}),
            )
        },
        // The refusal stays what it was; that it went unrecorded rides beside it.
        Case {
            arrange: refuse_audit,
            ..case(
                b"too short",
                StatusCode::BAD_REQUEST,
                serde_json::json!({"code": "size_mismatch", "audit": "unavailable"}),
            )
        },
        // An image, and a normalizer that could not do its work.
        Case {
            media_type: "image/png",
            ..case(
                BYTES,
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({"code": "storage_unavailable"}),
            )
        },
    ];
    for Case {
        media_type,
        sent,
        declared,
        arrange,
        status,
        body,
    } in cases
    {
        let fixture = fixture();
        let ticket = fixture.ticket(CONVERSATION, BYTES, media_type).await;
        arrange(&fixture);
        let mut request = headers(None, &[&ticket]);
        if let Some(declared) = declared {
            request.insert(header::CONTENT_LENGTH, HeaderValue::from(declared));
        }
        let response = handle_upload(route(&fixture), request, Body::from(sent)).await;
        assert_eq!(response.headers()[header::VARY], "origin");
        assert_eq!(answer(response).await, (status, body));
        assert!(fixture.store.held().is_empty());
    }
    for (error, status, code) in [
        // The words `conversation.send` uses for the same fact: this model is
        // offered no images. Nothing is wrong with the image.
        (
            NormalizeError::NotOffered,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "image_input_unsupported",
        ),
        (
            NormalizeError::Unsupported,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_image",
        ),
        (
            NormalizeError::TooLarge,
            StatusCode::PAYLOAD_TOO_LARGE,
            "image_too_large",
        ),
    ] {
        let fixture =
            Fixture::with_normalizer(AttachmentLimits::default(), StubNormalizer::failing(error));
        let ticket = fixture
            .ticket(OTHER_CONVERSATION, BYTES, "image/webp")
            .await;
        let response = handle_upload(
            route(&fixture),
            headers(None, &[&ticket]),
            Body::from(BYTES),
        )
        .await;
        assert_eq!(
            answer(response).await,
            (status, serde_json::json!({"code": code}))
        );
    }
}

#[tokio::test]
async fn a_body_that_breaks_off_and_a_gateway_with_no_agent_each_say_so() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let broken = stream::iter([
        Ok(BYTES[..10].to_vec()),
        Err(std::io::Error::other("connection reset")),
    ]);
    let response = handle_upload(
        route(&fixture),
        headers(None, &[&ticket]),
        Body::from_stream(broken),
    )
    .await;
    assert_eq!(
        answer(response).await,
        (
            StatusCode::BAD_REQUEST,
            serde_json::json!({"code": "upload_interrupted"})
        )
    );

    let response = handle_upload(
        State(UploadRoute::new(None)),
        headers(Some("tauri://localhost"), &[&"0".repeat(64)]),
        Body::from(BYTES),
    )
    .await;
    assert_eq!(
        response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "tauri://localhost"
    );
    assert_eq!(
        answer(response).await,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"code": "storage_unavailable"})
        )
    );
}

#[tokio::test]
async fn an_upload_whose_conversation_let_go_meanwhile_is_told_it_was_not_kept() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    // The creation record waits at the sink while the conversation closes.
    let open = fixture.audit.hold_after(0, false);
    let entered = fixture.audit.entered.notified();
    let state = route(&fixture);
    let upload = tokio::spawn(async move {
        handle_upload(state, headers(None, &[&ticket]), Body::from(BYTES)).await
    });
    entered.await;
    fixture
        .service
        .release(ReleaseRequest {
            organization_id: organization("org"),
            conversation_id: conversation(CONVERSATION),
            cause: ReleaseCause::ConversationClosed,
            principal_id: principal("owner"),
            surface_id: "panel".into(),
            correlation_id: "close-1".into(),
        })
        .await
        .unwrap();
    open.send(()).unwrap();
    assert_eq!(
        answer(upload.await.unwrap()).await,
        (
            StatusCode::CONFLICT,
            serde_json::json!({"code": "attachment_not_kept"})
        )
    );
}
