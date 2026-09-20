use crate::{
    attachments::{
        application::{
            AttachmentService, AuditDelivery, BodyInterrupted, PortFuture, UploadBody, UploadError,
            UploadRejection,
        },
        domain::Attachment,
    },
    server::entrypoint::origin,
};
use axum::{
    body::{Body, BodyDataStream},
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::StreamExt;
use serde_json::json;

/// The header that carries a ticket. A header, not a query parameter, so the
/// secret stays out of access logs, history, and referrers.
pub const TICKET_HEADER: &str = "x-nessa-upload-ticket";

/// How long a browser may remember the preflight answer, in seconds.
const PREFLIGHT_MAX_AGE: &str = "600";

/// All the upload route is given: the attachment service, when one is composed.
/// It has no use for sessions, credentials, or conversations, and cannot reach them.
#[derive(Clone)]
pub struct UploadRoute {
    attachments: Option<AttachmentService>,
}
impl UploadRoute {
    pub fn new(attachments: Option<AttachmentService>) -> Self {
        Self { attachments }
    }
}

/// `PUT /attachments`: the bytes of one upload, under the ticket
/// `attachment.begin` issued for them.
///
/// The route authenticates nobody. The ticket is the whole authority: the
/// authenticated socket decided who may upload what into which conversation,
/// and this route learns only what the ticket says. The body is streamed; it
/// is never collected here.
///
/// `200` answers with the file the conversation now keeps, which is what
/// `conversation.send` must refer to. For an image that is the normalized
/// image, so its digest, type, and size can all differ from what was sent.
pub(crate) async fn handle_upload(
    State(route): State<UploadRoute>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let allowed = match allowed_origin(&headers) {
        Allowed::No => return StatusCode::FORBIDDEN.into_response(),
        allowed => allowed,
    };
    let response = match &route.attachments {
        Some(attachments) => upload(attachments, &headers, body).await,
        // No agent is configured, so nothing is kept and nothing issued tickets.
        None => refusal(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", None),
    };
    with_cors(response, allowed)
}

async fn upload(attachments: &AttachmentService, headers: &HeaderMap, body: Body) -> Response {
    // Exactly one ticket. Two headers are not a choice this route makes.
    let mut tickets = headers.get_all(TICKET_HEADER).iter();
    let (Some(ticket), None) = (tickets.next(), tickets.next()) else {
        return rejected(UploadError::TicketInvalid);
    };
    let Ok(ticket) = ticket.to_str() else {
        return rejected(UploadError::TicketInvalid);
    };
    let declared_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let body = Box::new(HttpBody(body.into_data_stream()));
    match attachments.receive(ticket, declared_length, body).await {
        Ok(stored) => Json(stored_reference(&stored)).into_response(),
        Err(error) => rejected(error),
    }
}

/// The stored file, named the way `conversation.send` names an attachment.
fn stored_reference(stored: &Attachment) -> serde_json::Value {
    json!({
        "digest": stored.digest().to_string(),
        "mimeType": stored.media_type().as_str(),
        "size": stored.size(),
    })
}

/// The wire's answer to each refusal. A ticket that expired and one that never
/// existed are the same answer on purpose: the difference is for the audit
/// trail, not for whoever is holding a secret that does not work.
fn rejected(error: UploadError) -> Response {
    let (status, code, evidence) = match error {
        UploadError::TicketInvalid => (StatusCode::UNAUTHORIZED, "ticket_invalid", None),
        UploadError::TicketExpired { evidence } => {
            (StatusCode::UNAUTHORIZED, "ticket_invalid", Some(evidence))
        }
        UploadError::Busy => (
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            None,
        ),
        UploadError::AuditUnavailable { .. } => {
            (StatusCode::SERVICE_UNAVAILABLE, "audit_unavailable", None)
        }
        UploadError::Rejected { reason, evidence } => {
            let (status, code) = match reason {
                UploadRejection::SizeMismatch => (StatusCode::BAD_REQUEST, "size_mismatch"),
                UploadRejection::DigestMismatch => {
                    (StatusCode::UNPROCESSABLE_ENTITY, "digest_mismatch")
                }
                UploadRejection::BodyInterrupted => (StatusCode::BAD_REQUEST, "upload_interrupted"),
                UploadRejection::DeadlineElapsed => (StatusCode::REQUEST_TIMEOUT, "upload_timeout"),
                UploadRejection::UnsupportedImage => {
                    (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_image")
                }
                UploadRejection::ImageTooLarge => {
                    (StatusCode::PAYLOAD_TOO_LARGE, "image_too_large")
                }
                UploadRejection::StorageUnavailable | UploadRejection::NormalizationFailed => {
                    (StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable")
                }
            };
            (status, code, Some(evidence))
        }
    };
    refusal(status, code, evidence)
}

/// `{"code": …}`, and `"audit": "unavailable"` beside it when the refusal
/// itself could not be recorded. The refusal stays the primary failure; the
/// lost evidence is reported with it rather than instead of it.
fn refusal(status: StatusCode, code: &'static str, evidence: Option<AuditDelivery>) -> Response {
    let body = match evidence {
        Some(AuditDelivery::Unavailable) => json!({"code": code, "audit": "unavailable"}),
        Some(AuditDelivery::Recorded) | None => json!({"code": code}),
    };
    (status, Json(body)).into_response()
}

/// `OPTIONS /attachments`: a browser asks before it sends a `PUT` carrying a
/// header of its own from another origin, which is every upload from the
/// desktop shell and from the development server.
pub(crate) async fn handle_preflight(headers: HeaderMap) -> Response {
    let allowed = match allowed_origin(&headers) {
        Allowed::No => return StatusCode::FORBIDDEN.into_response(),
        allowed => allowed,
    };
    let mut response = StatusCode::NO_CONTENT.into_response();
    let answer = response.headers_mut();
    answer.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("PUT"),
    );
    answer.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type, x-nessa-upload-ticket"),
    );
    answer.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static(PREFLIGHT_MAX_AGE),
    );
    with_cors(response, allowed)
}

/// Every answer varies by origin, and a trusted page is told it may read this one.
fn with_cors(mut response: Response, allowed: Allowed) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::VARY, HeaderValue::from_static("origin"));
    if let Allowed::Cross(origin) = allowed {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    response
}

/// Who asked, and whether they may send and read.
enum Allowed {
    /// No `Origin`: a native client, not a page.
    Same,
    /// A page on an origin this server trusts, echoed back exactly. Never `*`.
    Cross(HeaderValue),
    /// A page on any other origin.
    No,
}

/// The same rule `/session` applies: a present `Origin` must be one this
/// server trusts. A ticket is a bearer secret, and a page on another origin
/// holding one is not a caller this gateway serves.
fn allowed_origin(headers: &HeaderMap) -> Allowed {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Allowed::Same;
    };
    if origin::is_trusted_ws_origin(origin) {
        Allowed::Cross(origin.clone())
    } else {
        Allowed::No
    }
}

/// An HTTP request body as the application's chunk source.
struct HttpBody(BodyDataStream);
impl UploadBody for HttpBody {
    fn next(&mut self) -> PortFuture<'_, Option<Vec<u8>>, BodyInterrupted> {
        Box::pin(async move {
            match self.0.next().await {
                None => Ok(None),
                Some(Ok(chunk)) => Ok(Some(chunk.to_vec())),
                Some(Err(_)) => Err(BodyInterrupted),
            }
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/attachments/http.rs"]
mod tests;
