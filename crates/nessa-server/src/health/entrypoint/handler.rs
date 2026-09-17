use crate::desktop_runtime::domain::RunningRuntime;
use axum::{
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
/// HTTP liveness includes the verified generation only for a composed desktop runtime.
pub(crate) async fn handle_http_health(identity: Option<Extension<RunningRuntime>>) -> Response {
    let mut response = StatusCode::OK.into_response();
    if let Some(Extension(value)) = identity {
        for (name, text) in [
            (
                "x-nessa-runtime-fingerprint",
                value.fingerprint().as_str().to_owned(),
            ),
            (
                "x-nessa-runtime-instance",
                value.instance().as_str().to_owned(),
            ),
            ("x-nessa-process-id", value.process_id().to_string()),
            (
                "x-nessa-service-generation",
                value.generation().as_str().to_owned(),
            ),
        ] {
            response.headers_mut().insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(&text).expect("validated desktop runtime header"),
            );
        }
    }
    response
}
#[cfg(test)]
#[path = "../../../tests/desktop_runtime/health.rs"]
mod tests;
