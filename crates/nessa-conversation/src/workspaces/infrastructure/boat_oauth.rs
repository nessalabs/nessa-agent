//! Which Boat OAuth app this gateway signs in through, and Boat's OAuth
//! endpoints over HTTPS (or HTTP to a fake on this machine, which
//! [`BoatApp`] alone admits).
//!
//! The released app is `protocol/defaults/boat-app.json`. Its client id is
//! public: the app is registered without a secret (`boat app create
//! --no-secret`), as Boat asks of desktop apps, and every exchange proves
//! itself with PKCE instead. An empty client id means this build offers no
//! Sign in with Boat, only a pasted token. Composition may read another
//! file in its place (`NESSA_BOAT_APP`) to point a check at a fake Boat.
//!
//! What Boat's answers mean is decided by the pure functions below
//! ([`tokens`], [`refusal`], [`login`]); the client only carries bytes. A
//! redirect is a status, never followed.
use crate::workspaces::application::{BoatAuthorization, BoatTokens, GrantFailure};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

const RELEASED: &str = include_str!("../../../../../protocol/defaults/boat-app.json");
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Every answer here is well under this; a larger one is not Boat's.
const BODY_LIMIT: usize = 65_536;
/// What Nessa does with a Boat account (`docs/design/boat-sign-in.md`):
/// read it, keep its `nessa` environment in the one posture Nessa admits,
/// make, wake, stop and roll back its own VMs, and reach them.
pub const SCOPES: &str = "account.read environment.read environment.write snapshot.read \
sandbox.read sandbox.create sandbox.resume sandbox.stop sandbox.delete-own exec ssh";

/// A Boat OAuth app's public facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoatApp {
    /// `boatapp_…`, or empty when this build names no app.
    pub client_id: String,
    /// boat.dev: where the person approves the app, and Boat's issuer.
    pub web_url: Url,
    /// boat.dev/api/v1: where the signed-in account is read, and where
    /// every VM call goes.
    pub api_url: Url,
}

/// Why a description of the app was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoatAppProblem {
    Malformed,
    Identity,
    Url,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Described {
    client_id: String,
    web_url: String,
    api_url: String,
}

impl BoatApp {
    pub fn released() -> Self {
        Self::parse(RELEASED.as_bytes()).expect("protocol/defaults/boat-app.json is valid")
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, BoatAppProblem> {
        let described: Described =
            serde_json::from_slice(bytes).map_err(|_| BoatAppProblem::Malformed)?;
        if !described
            .client_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            || described.client_id.len() > 256
        {
            return Err(BoatAppProblem::Identity);
        }
        Ok(Self {
            client_id: described.client_id,
            web_url: base(&described.web_url)?,
            api_url: base(&described.api_url)?,
        })
    }
    /// The base every Boat API call goes to, without its trailing `/`, as
    /// the Boat client takes it.
    pub fn api_base(&self) -> String {
        self.api_url.as_str().trim_end_matches('/').to_owned()
    }
    fn oauth(&self, endpoint: &str) -> String {
        format!("{}api/boat/oauth2/{endpoint}", self.web_url.as_str())
    }
}

/// A base URL, ending in `/`: https anywhere, or http only to this machine.
fn base(value: &str) -> Result<Url, BoatAppProblem> {
    let mut url = Url::parse(value).map_err(|_| BoatAppProblem::Url)?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(name)) => name == "localhost",
        None => false,
    };
    let scheme = match url.scheme() {
        "https" => true,
        "http" => loopback,
        _ => false,
    };
    if !scheme
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(BoatAppProblem::Url);
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

/// Boat's OAuth endpoints for one app.
pub struct BoatOAuthHttps {
    app: BoatApp,
    client: reqwest::Client,
}

impl BoatOAuthHttps {
    /// `None` when no HTTP client can be built on this machine.
    pub fn new(app: BoatApp) -> Option<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CALL_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Nessa")
            .build()
            .ok()
            .map(|client| Self { app, client })
    }

    async fn post_form(&self, endpoint: &str, fields: &[(&str, &str)]) -> Result<Answer, ()> {
        let body = Zeroizing::new(
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(fields)
                .finish(),
        );
        let request = self
            .client
            .post(self.app.oauth(endpoint))
            .header(reqwest::header::ACCEPT, "application/json")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body.to_string());
        send(request).await
    }

    async fn token(&self, fields: &[(&str, &str)]) -> Result<BoatTokens, GrantFailure> {
        match self.post_form("token", fields).await {
            Ok(answer) => tokens(answer.status, &answer.body),
            Err(()) => Err(GrantFailure::Unreachable),
        }
    }
}

struct Answer {
    status: u16,
    body: Zeroizing<Vec<u8>>,
}

async fn send(request: reqwest::RequestBuilder) -> Result<Answer, ()> {
    let mut response = request.send().await.map_err(|_| ())?;
    let status = response.status().as_u16();
    let mut body = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if body.len() + chunk.len() > BODY_LIMIT {
            return Err(());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Answer { status, body })
}

#[async_trait]
impl BoatAuthorization for BoatOAuthHttps {
    fn authorize_url(&self, redirect_uri: &str, state: &str, challenge: &str) -> Option<String> {
        if self.app.client_id.is_empty() {
            return None;
        }
        let mut url = Url::parse(&self.app.oauth("authorize")).ok()?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.app.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", SCOPES)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        Some(url.into())
    }
    fn issuer(&self) -> String {
        self.app.web_url.as_str().trim_end_matches('/').to_owned()
    }
    async fn exchange(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
    ) -> Result<BoatTokens, GrantFailure> {
        // A code works once: never asked for twice.
        self.token(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("client_id", &self.app.client_id),
        ])
        .await
    }
    async fn refresh(&self, refresh: &str) -> Result<BoatTokens, GrantFailure> {
        let fields = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", self.app.client_id.as_str()),
        ];
        match self.token(&fields).await {
            // Boat answers the same request again within a minute, so an
            // answer lost on the way back is asked for once more rather
            // than lost with the refresh token it spent.
            Err(GrantFailure::Unreachable) => self.token(&fields).await,
            answered => answered,
        }
    }
    async fn revoke(&self, refresh: &str) {
        let _ = self
            .post_form(
                "revoke",
                &[("token", refresh), ("client_id", &self.app.client_id)],
            )
            .await;
    }
    async fn login(&self, access: &str) -> Result<String, GrantFailure> {
        let request = self
            .client
            .get(format!("{}me", self.app.api_url.as_str()))
            .header(reqwest::header::ACCEPT, "application/json")
            .bearer_auth(access);
        match send(request).await {
            Ok(answer) => login(answer.status, &answer.body),
            Err(()) => Err(GrantFailure::Unreachable),
        }
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: String,
}

/// What a token endpoint answer means.
pub fn tokens(status: u16, body: &[u8]) -> Result<BoatTokens, GrantFailure> {
    if status != 200 {
        return Err(refusal(status, body));
    }
    let answer: TokenAnswer = serde_json::from_slice(body).map_err(|_| GrantFailure::Rejected)?;
    let shaped = |value: &str| {
        !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_whitespace)
    };
    if !answer.token_type.eq_ignore_ascii_case("bearer")
        || !shaped(&answer.access_token)
        || !shaped(&answer.refresh_token)
        || answer.expires_in == 0
    {
        return Err(GrantFailure::Rejected);
    }
    Ok(BoatTokens {
        access: Zeroizing::new(answer.access_token),
        refresh: Zeroizing::new(answer.refresh_token),
        expires_in: answer.expires_in,
    })
}

/// A refusal: `invalid_grant` means the code or refresh token is spent
/// for good; a server failure may pass; anything else is the app's.
pub fn refusal(status: u16, body: &[u8]) -> GrantFailure {
    #[derive(Deserialize)]
    struct Refusal {
        error: String,
    }
    if status >= 500 || status == 429 {
        return GrantFailure::Unreachable;
    }
    match serde_json::from_slice::<Refusal>(body) {
        Ok(refusal) if refusal.error == "invalid_grant" => GrantFailure::Refused,
        _ => GrantFailure::Rejected,
    }
}

/// The login `GET /me` names: the account's login, or its email when it
/// has no login.
pub fn login(status: u16, body: &[u8]) -> Result<String, GrantFailure> {
    #[derive(Deserialize)]
    struct Me {
        user: User,
    }
    #[derive(Deserialize)]
    struct User {
        login: Option<String>,
        email: Option<String>,
    }
    match status {
        200 => {}
        401 | 403 => return Err(GrantFailure::Rejected),
        _ => return Err(GrantFailure::Unreachable),
    }
    let me: Me = serde_json::from_slice(body).map_err(|_| GrantFailure::Rejected)?;
    [me.user.login, me.user.email]
        .into_iter()
        .flatten()
        .find(|name| !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control))
        .ok_or(GrantFailure::Rejected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_released_app_parses() {
        let app = BoatApp::released();
        assert_eq!(app.web_url.as_str(), "https://boat.dev/");
        assert_eq!(app.api_base(), "https://boat.dev/api/v1");
    }

    #[test]
    fn an_app_is_https_or_this_machine() {
        let parse = |web: &str| {
            BoatApp::parse(
                format!(r#"{{"clientId":"boatapp_x","webUrl":"{web}","apiUrl":"{web}/api/v1"}}"#)
                    .as_bytes(),
            )
        };
        assert!(parse("https://boat.dev").is_ok());
        assert!(parse("http://127.0.0.1:4000").is_ok());
        assert_eq!(parse("http://boat.dev"), Err(BoatAppProblem::Url));
        assert_eq!(
            BoatApp::parse(
                br#"{"clientId":"a b","webUrl":"https://boat.dev","apiUrl":"https://boat.dev"}"#
            ),
            Err(BoatAppProblem::Identity)
        );
    }

    #[test]
    fn no_client_id_offers_no_sign_in() {
        let app = BoatApp::parse(
            br#"{"clientId":"","webUrl":"https://boat.dev","apiUrl":"https://boat.dev/api/v1"}"#,
        )
        .unwrap();
        let oauth = BoatOAuthHttps::new(app).unwrap();
        assert!(oauth
            .authorize_url("http://localhost:1/boat/callback", "s", "c")
            .is_none());
    }

    #[test]
    fn the_authorize_url_carries_pkce_and_scopes() {
        let app = BoatApp::parse(
            br#"{"clientId":"boatapp_abc","webUrl":"https://boat.dev","apiUrl":"https://boat.dev/api/v1"}"#,
        )
        .unwrap();
        let oauth = BoatOAuthHttps::new(app).unwrap();
        let url = Url::parse(
            &oauth
                .authorize_url("http://localhost:5123/boat/callback", "st", "ch")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(url.path(), "/api/boat/oauth2/authorize");
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["client_id"], "boatapp_abc");
        assert_eq!(pairs["redirect_uri"], "http://localhost:5123/boat/callback");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["code_challenge"], "ch");
        assert_eq!(pairs["state"], "st");
        assert_eq!(pairs["scope"], SCOPES);
        assert_eq!(oauth.issuer(), "https://boat.dev");
    }

    #[test]
    fn token_answers_read_as_boat_documents_them() {
        let ok = br#"{"access_token":"boat_a","token_type":"Bearer","expires_in":3600,"refresh_token":"boatrt_r","scope":"exec"}"#;
        let tokens = tokens(200, ok).unwrap();
        assert_eq!(tokens.access.as_str(), "boat_a");
        assert_eq!(tokens.refresh.as_str(), "boatrt_r");
        assert_eq!(tokens.expires_in, 3600);
        assert_eq!(
            super::tokens(
                400,
                br#"{"error":"invalid_grant","error_description":"used"}"#
            )
            .err(),
            Some(GrantFailure::Refused)
        );
        assert_eq!(
            super::tokens(401, br#"{"error":"invalid_client"}"#).err(),
            Some(GrantFailure::Rejected)
        );
        assert_eq!(
            super::tokens(503, b"").err(),
            Some(GrantFailure::Unreachable)
        );
        assert_eq!(
            super::tokens(200, br#"{"access_token":"a b","token_type":"Bearer","expires_in":1,"refresh_token":"r"}"#).err(),
            Some(GrantFailure::Rejected)
        );
    }

    #[test]
    fn the_account_is_its_login_or_its_email() {
        assert_eq!(
            login(
                200,
                br#"{"ok":true,"type":"user.info","user":{"login":"octo","email":"o@x"}}"#
            ),
            Ok("octo".into())
        );
        assert_eq!(
            login(
                200,
                br#"{"ok":true,"type":"user.info","user":{"email":"o@x"}}"#
            ),
            Ok("o@x".into())
        );
        assert_eq!(login(401, b"{}"), Err(GrantFailure::Rejected));
    }
}
