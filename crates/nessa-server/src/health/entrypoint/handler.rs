/// HTTP probe used by smoke tests and load balancers.
pub async fn handle_http_health() -> axum::http::StatusCode {
    axum::http::StatusCode::OK
}
