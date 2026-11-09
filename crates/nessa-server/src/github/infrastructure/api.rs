//! The device flow and the account read, over HTTPS to GitHub (or HTTP to a
//! fake on this machine, which [`GithubApp`] alone admits).
//!
//! What GitHub's answers mean is decided by the pure functions below
//! ([`device_authorization`], [`poll_answer`], [`account`]), each tested on
//! GitHub's documented shapes; the client only carries bytes. A redirect is
//! a status, never followed.
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use super::app::GithubApp;
use crate::agent_install::infrastructure::https_archives::install_tls_backend;
use crate::github::application::{
    ApiFailure, DeviceAuthorization, DeviceCode, GithubApi, GithubReadFailure, PollAnswer,
};
use crate::github::domain::{AccessToken, GithubAccount, GithubReadKind, GithubRepository};

/// Shorter than [`CALL_TIMEOUT`], so a connect that never completes fails
/// as itself.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Every answer here is a few hundred bytes; a larger one is not GitHub's.
const BODY_LIMIT: usize = 65_536;
/// GitHub's own default when an answer names no interval.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);
/// The fastest the gateway polls, whatever an answer asks.
const MIN_INTERVAL: Duration = Duration::from_secs(1);
/// What GitHub adds to the interval on `slow_down` when it names none.
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);
/// The longest values the wire carries (`GithubCode`, `GithubAccount` in
/// `protocol/product/v1.json`); a longer one is not an answer this reads.
const MAX_USER_CODE_BYTES: usize = 64;
const MAX_URI_BYTES: usize = 2048;
const MAX_LOGIN_BYTES: usize = 256;
/// Never shown; bounded so the poll's form stays small.
const MAX_DEVICE_CODE_BYTES: usize = 1024;
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

pub struct GithubHttps {
    app: GithubApp,
    client: reqwest::Client,
}

impl GithubHttps {
    /// `None` when no HTTP client can be built on this machine.
    pub fn new(app: GithubApp) -> Option<Self> {
        install_tls_backend();
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CALL_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Nessa")
            .build()
            .ok()
            .map(|client| Self { app, client })
    }

    async fn post_form(&self, url: String, fields: &[(&str, &str)]) -> Result<Answer, ApiFailure> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        let request = self
            .client
            .post(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body);
        self.send(request).await
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Answer, ApiFailure> {
        let mut response = request.send().await.map_err(|_| ApiFailure::Unreachable)?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ApiFailure::Unreachable)?
        {
            if body.len().saturating_add(chunk.len()) > BODY_LIMIT {
                return Err(ApiFailure::Malformed);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Answer { status, body })
    }
}

/// One HTTP answer, before it means anything.
pub(super) struct Answer {
    pub(super) status: u16,
    pub(super) body: Vec<u8>,
}

#[async_trait]
impl GithubApi for GithubHttps {
    async fn request_device_code(&self) -> Result<DeviceAuthorization, ApiFailure> {
        let url = self.app.at(&self.app.web_url, "login/device/code");
        let answer = self
            .post_form(url, &[("client_id", &self.app.client_id)])
            .await?;
        device_authorization(&answer)
    }

    async fn poll(&self, device_code: &DeviceCode) -> Result<PollAnswer, ApiFailure> {
        let url = self.app.at(&self.app.web_url, "login/oauth/access_token");
        let answer = self
            .post_form(
                url,
                &[
                    ("client_id", &self.app.client_id),
                    ("device_code", &device_code.0),
                    ("grant_type", DEVICE_GRANT),
                ],
            )
            .await?;
        poll_answer(&answer)
    }

    async fn account(&self, token: &AccessToken) -> Result<GithubAccount, ApiFailure> {
        let url = self.app.at(&self.app.api_url, "user");
        let request = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .bearer_auth(token.expose());
        account(&self.send(request).await?)
    }

    async fn read_repository(
        &self,
        token: &AccessToken,
        repository: &GithubRepository,
        kind: GithubReadKind,
    ) -> Result<Value, GithubReadFailure> {
        let path = repository
            .path(kind)
            .map_err(|_| GithubReadFailure::InvalidAnswer)?;
        let url = self.app.at(&self.app.api_url, &path);
        let request = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .bearer_auth(token.expose());
        let answer = self.send(request).await.map_err(|failure| match failure {
            ApiFailure::Unreachable => GithubReadFailure::Unreachable,
            ApiFailure::Malformed => GithubReadFailure::InvalidAnswer,
            ApiFailure::Rejected | ApiFailure::Unauthorized => GithubReadFailure::Unauthorized,
        })?;
        repository_answer(&answer)
    }
}

fn repository_answer(answer: &Answer) -> Result<Value, GithubReadFailure> {
    match answer.status {
        200 => serde_json::from_slice(&answer.body).map_err(|_| GithubReadFailure::InvalidAnswer),
        401 | 403 => Err(GithubReadFailure::Unauthorized),
        404 => Err(GithubReadFailure::NotFound),
        429 | 500..=599 => Err(GithubReadFailure::Unreachable),
        _ => Err(GithubReadFailure::InvalidAnswer),
    }
}

/// A server failure or a rate limit (403 and 429, as GitHub sends them) is no
/// answer yet; any other refusal is GitHub's refusal of the app.
fn refused(status: u16) -> ApiFailure {
    if status >= 500 || status == 403 || status == 429 {
        ApiFailure::Unreachable
    } else {
        ApiFailure::Rejected
    }
}

fn json(answer: &Answer) -> Result<Value, ApiFailure> {
    serde_json::from_slice(&answer.body).map_err(|_| ApiFailure::Malformed)
}

fn seconds(value: &Value) -> Option<Duration> {
    value.as_u64().map(Duration::from_secs)
}

pub(super) fn device_authorization(answer: &Answer) -> Result<DeviceAuthorization, ApiFailure> {
    if answer.status != 200 {
        return Err(refused(answer.status));
    }
    let body = json(answer)?;
    if body.get("error").is_some() {
        return Err(ApiFailure::Rejected);
    }
    let text = |key: &str, limit: usize| {
        body.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= limit)
            .map(str::to_owned)
            .ok_or(ApiFailure::Malformed)
    };
    let verification_uri = text("verification_uri", MAX_URI_BYTES)?;
    if !verification_uri.starts_with("https://") && !verification_uri.starts_with("http://") {
        return Err(ApiFailure::Malformed);
    }
    Ok(DeviceAuthorization {
        device_code: DeviceCode(text("device_code", MAX_DEVICE_CODE_BYTES)?),
        user_code: text("user_code", MAX_USER_CODE_BYTES)?,
        verification_uri,
        expires_in: body
            .get("expires_in")
            .and_then(seconds)
            .ok_or(ApiFailure::Malformed)?,
        interval: body
            .get("interval")
            .and_then(seconds)
            .unwrap_or(DEFAULT_INTERVAL)
            .max(MIN_INTERVAL),
    })
}

pub(super) fn poll_answer(answer: &Answer) -> Result<PollAnswer, ApiFailure> {
    if answer.status != 200 {
        return Err(refused(answer.status));
    }
    let body = json(answer)?;
    if let Some(token) = body.get("access_token").and_then(Value::as_str) {
        return AccessToken::new(token.to_owned())
            .map(PollAnswer::Granted)
            .ok_or(ApiFailure::Malformed);
    }
    match body.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Ok(PollAnswer::Pending),
        Some("slow_down") => Ok(PollAnswer::SlowDown {
            interval: body
                .get("interval")
                .and_then(seconds)
                .unwrap_or(DEFAULT_INTERVAL + SLOW_DOWN_STEP),
        }),
        Some("expired_token") => Ok(PollAnswer::Expired),
        Some("access_denied") => Ok(PollAnswer::Denied),
        Some(_) => Err(ApiFailure::Rejected),
        None => Err(ApiFailure::Malformed),
    }
}

pub(super) fn account(answer: &Answer) -> Result<GithubAccount, ApiFailure> {
    match answer.status {
        200 => {}
        401 => return Err(ApiFailure::Unauthorized),
        status => return Err(refused(status)),
    }
    let body = json(answer)?;
    Ok(GithubAccount {
        id: body
            .get("id")
            .and_then(Value::as_u64)
            .ok_or(ApiFailure::Malformed)?,
        login: body
            .get("login")
            .and_then(Value::as_str)
            .filter(|login| !login.is_empty() && login.len() <= MAX_LOGIN_BYTES)
            .ok_or(ApiFailure::Malformed)?
            .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answer(status: u16, body: Value) -> Answer {
        Answer {
            status,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    #[test]
    fn a_device_code_answer_is_read_as_github_documents_it() {
        let read = device_authorization(&answer(
            200,
            json!({"device_code": "dc", "user_code": "ABCD-1234",
                   "verification_uri": "https://github.com/login/device",
                   "expires_in": 899, "interval": 5}),
        ))
        .unwrap();
        assert_eq!(read.device_code, DeviceCode("dc".into()));
        assert_eq!(read.user_code, "ABCD-1234");
        assert_eq!(read.expires_in, Duration::from_secs(899));
        assert_eq!(read.interval, Duration::from_secs(5));
        let eager = device_authorization(&answer(
            200,
            json!({"device_code": "dc", "user_code": "ABCD-1234",
                   "verification_uri": "https://github.com/login/device",
                   "expires_in": 899, "interval": 0}),
        ))
        .unwrap();
        assert_eq!(eager.interval, MIN_INTERVAL, "never a busy loop");
        // The device flow switched off for the app is a refusal of the app.
        assert_eq!(
            device_authorization(&answer(400, json!({"error": "device_flow_disabled"}))),
            Err(ApiFailure::Rejected)
        );
        assert_eq!(
            device_authorization(&answer(200, json!({"error": "unauthorized_client"}))),
            Err(ApiFailure::Rejected)
        );
        assert_eq!(
            device_authorization(&answer(503, json!({}))),
            Err(ApiFailure::Unreachable)
        );
        assert_eq!(
            device_authorization(&answer(200, json!({"user_code": "x"}))),
            Err(ApiFailure::Malformed)
        );
    }

    #[test]
    fn each_poll_answer_means_what_github_says() {
        let read = |body| poll_answer(&answer(200, body));
        assert_eq!(
            read(json!({"error": "authorization_pending"})),
            Ok(PollAnswer::Pending)
        );
        assert_eq!(
            read(json!({"error": "slow_down", "interval": 10})),
            Ok(PollAnswer::SlowDown {
                interval: Duration::from_secs(10)
            })
        );
        assert_eq!(
            read(json!({"error": "slow_down"})),
            Ok(PollAnswer::SlowDown {
                interval: Duration::from_secs(10)
            })
        );
        assert_eq!(
            read(json!({"error": "expired_token"})),
            Ok(PollAnswer::Expired)
        );
        assert_eq!(
            read(json!({"error": "access_denied"})),
            Ok(PollAnswer::Denied)
        );
        assert_eq!(
            read(json!({"error": "incorrect_client_credentials"})),
            Err(ApiFailure::Rejected)
        );
        assert_eq!(
            read(json!({"access_token": "ghu_x", "token_type": "bearer"})),
            Ok(PollAnswer::Granted(
                AccessToken::new("ghu_x".into()).unwrap()
            ))
        );
        assert_eq!(
            read(json!({"access_token": ""})),
            Err(ApiFailure::Malformed)
        );
        assert_eq!(read(json!({})), Err(ApiFailure::Malformed));
    }

    #[test]
    fn the_account_is_its_id_and_login_and_a_refused_token_says_so() {
        assert_eq!(
            account(&answer(200, json!({"id": 7, "login": "octo"}))),
            Ok(GithubAccount {
                id: 7,
                login: "octo".into()
            })
        );
        assert_eq!(
            account(&answer(401, json!({}))),
            Err(ApiFailure::Unauthorized)
        );
        assert_eq!(
            account(&answer(502, json!({}))),
            Err(ApiFailure::Unreachable)
        );
        assert_eq!(
            account(&answer(200, json!({"login": "octo"}))),
            Err(ApiFailure::Malformed)
        );
        // A rate limit is no answer yet, never a refused token.
        for status in [403, 429] {
            assert_eq!(
                account(&answer(status, json!({}))),
                Err(ApiFailure::Unreachable)
            );
            assert_eq!(
                poll_answer(&answer(status, json!({}))),
                Err(ApiFailure::Unreachable)
            );
        }
        // Longer than the wire carries is not an answer.
        assert_eq!(
            account(&answer(200, json!({"id": 7, "login": "o".repeat(257)}))),
            Err(ApiFailure::Malformed)
        );
        assert_eq!(
            device_authorization(&answer(
                200,
                json!({"device_code": "dc", "user_code": "A".repeat(65),
                       "verification_uri": "https://github.com/login/device",
                       "expires_in": 899}),
            )),
            Err(ApiFailure::Malformed)
        );
    }

    #[test]
    fn repository_reads_refuse_redirects_errors_and_malformed_answers() {
        assert_eq!(
            repository_answer(&answer(200, json!({"id":7}))),
            Ok(json!({"id":7}))
        );
        for (status, failure) in [
            (302, GithubReadFailure::InvalidAnswer),
            (401, GithubReadFailure::Unauthorized),
            (403, GithubReadFailure::Unauthorized),
            (404, GithubReadFailure::NotFound),
            (429, GithubReadFailure::Unreachable),
            (502, GithubReadFailure::Unreachable),
        ] {
            assert_eq!(repository_answer(&answer(status, json!({}))), Err(failure));
        }
        assert_eq!(
            repository_answer(&Answer {
                status: 200,
                body: b"not JSON".to_vec()
            }),
            Err(GithubReadFailure::InvalidAnswer)
        );
    }
}
