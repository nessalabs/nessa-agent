//! OpenAI's sign-in over HTTPS (or HTTP to a fake on this machine, which
//! [`ChatgptSignInConfig`] alone admits), and the ID token's verification
//! against OpenAI's published keys.
//!
//! What OpenAI's answers mean is decided by the pure functions below
//! ([`token_answer`], [`refresh_failure`], [`verify_id_token`]), each tested
//! on the documented shapes; the client only carries bytes. A redirect is a
//! status, never followed.
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::logins::chatgpt::domain::{Secret, AGENT_NAME, RESOURCE, SCOPES};
use crate::logins::chatgpt::ports::{
    AuthorityFailure, Authorize, ChatgptAuthority, Identity, RefreshFailure, TokenAnswer,
};
use nessa_agent_install::infrastructure::https_archives::install_tls_backend;

const RELEASED: &str = include_str!("../../../../../../protocol/defaults/chatgpt-sign-in.json");

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// A token answer is a few kilobytes and OpenAI's key set smaller; a larger
/// answer is not one of theirs.
const BODY_LIMIT: usize = 256 * 1024;
/// Most an ID token's clock may be ahead of this one.
const CLOCK_SKEW_MS: u64 = 5 * 60 * 1000;

/// OpenAI's sign-in addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatgptSignInConfig {
    pub issuer: String,
    pub authorize_url: Url,
    pub token_url: Url,
    pub revoke_url: Url,
    pub jwks_url: Url,
}

/// A description of the addresses that was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigProblem {
    Malformed,
    /// A URL that is not https, or http to this machine, or that carries a
    /// user, a query or a fragment.
    Url,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Described {
    issuer: String,
    authorize_url: String,
    token_url: String,
    revoke_url: String,
    jwks_url: String,
}

fn address(value: &str) -> Result<Url, ConfigProblem> {
    let url = Url::parse(value).map_err(|_| ConfigProblem::Url)?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let scheme = url.scheme() == "https" || (url.scheme() == "http" && local);
    if !scheme
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigProblem::Url);
    }
    Ok(url)
}

impl ChatgptSignInConfig {
    /// OpenAI's own addresses.
    pub fn released() -> Self {
        Self::parse(RELEASED.as_bytes()).expect("protocol/defaults/chatgpt-sign-in.json is valid")
    }

    /// The addresses `bytes` describe.
    ///
    /// # Errors
    /// [`ConfigProblem`].
    pub fn parse(bytes: &[u8]) -> Result<Self, ConfigProblem> {
        let described: Described =
            serde_json::from_slice(bytes).map_err(|_| ConfigProblem::Malformed)?;
        address(&described.issuer)?;
        Ok(Self {
            issuer: described.issuer,
            authorize_url: address(&described.authorize_url)?,
            token_url: address(&described.token_url)?,
            revoke_url: address(&described.revoke_url)?,
            jwks_url: address(&described.jwks_url)?,
        })
    }
}

pub struct OpenaiSignIn {
    config: ChatgptSignInConfig,
    client: reqwest::Client,
}

impl OpenaiSignIn {
    /// `None` when no HTTP client can be built on this machine.
    pub fn new(config: ChatgptSignInConfig) -> Option<Self> {
        install_tls_backend();
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CALL_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(AGENT_NAME)
            .build()
            .ok()
            .map(|client| Self { config, client })
    }

    async fn post_form(&self, url: &Url, fields: &[(&str, &str)]) -> Option<Answer> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        let request = self
            .client
            .post(url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body);
        self.send(request).await
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Option<Answer> {
        let mut response = request.send().await.ok()?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if body.len().saturating_add(chunk.len()) > BODY_LIMIT {
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        Some(Answer { status, body })
    }
}

/// One HTTP answer, before it means anything.
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

#[async_trait]
impl ChatgptAuthority for OpenaiSignIn {
    fn authorize_url(&self, request: &Authorize<'_>) -> String {
        authorize_url(&self.config.authorize_url, request)
    }

    async fn exchange(
        &self,
        client_id: &str,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<TokenAnswer, AuthorityFailure> {
        let answer = self
            .post_form(
                &self.config.token_url,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", client_id),
                    ("code", code),
                    ("code_verifier", verifier),
                    ("redirect_uri", redirect_uri),
                    ("resource", RESOURCE),
                ],
            )
            .await
            .ok_or(AuthorityFailure::Unreachable)?;
        if answer.status >= 500 {
            return Err(AuthorityFailure::Unreachable);
        }
        if answer.status != 200 {
            return Err(AuthorityFailure::Refused);
        }
        token_answer(&answer.body).ok_or(AuthorityFailure::Malformed)
    }

    async fn refresh(
        &self,
        client_id: &str,
        refresh: &Secret,
    ) -> Result<TokenAnswer, RefreshFailure> {
        let answer = self
            .post_form(
                &self.config.token_url,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", client_id),
                    ("refresh_token", refresh.expose()),
                    ("resource", RESOURCE),
                ],
            )
            .await
            .ok_or(RefreshFailure::Unreachable)?;
        if answer.status == 200 {
            return token_answer(&answer.body).ok_or(RefreshFailure::Unreachable);
        }
        Err(refresh_failure(&answer))
    }

    async fn revoke(&self, client_id: &str, refresh: &Secret) -> Result<(), AuthorityFailure> {
        let answer = self
            .post_form(
                &self.config.revoke_url,
                &[
                    ("token", refresh.expose()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", client_id),
                ],
            )
            .await
            .ok_or(AuthorityFailure::Unreachable)?;
        match answer.status {
            200 => Ok(()),
            500.. => Err(AuthorityFailure::Unreachable),
            _ => Err(AuthorityFailure::Refused),
        }
    }

    async fn identity(
        &self,
        id_token: &Secret,
        client_id: &str,
        nonce: &str,
        now_ms: u64,
    ) -> Result<Identity, AuthorityFailure> {
        let answer = self
            .send(
                self.client
                    .get(self.config.jwks_url.clone())
                    .header(reqwest::header::ACCEPT, "application/json"),
            )
            .await
            .ok_or(AuthorityFailure::Unreachable)?;
        if answer.status != 200 {
            return Err(AuthorityFailure::Unreachable);
        }
        let keys: Value =
            serde_json::from_slice(&answer.body).map_err(|_| AuthorityFailure::Malformed)?;
        verify_id_token(
            id_token.expose(),
            &keys,
            &self.config.issuer,
            client_id,
            nonce,
            now_ms,
        )
        .ok_or(AuthorityFailure::Malformed)
    }
}

/// Where the browser is sent for `request`, each value URL-encoded.
pub fn authorize_url(base: &Url, request: &Authorize<'_>) -> String {
    let mut url = base.clone();
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("client_id", request.client_id);
        if request.register {
            query.append_pair("agent_name_hint", AGENT_NAME);
        }
        query.append_pair("ext_agent_host_id", request.host_id);
        if let Some(hint) = request.id_token_hint {
            query.append_pair("id_token_hint", hint.expose());
        }
        if let Some(hint) = request.login_hint.filter(|hint| !hint.is_empty()) {
            query.append_pair("login_hint", hint);
        }
        query
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", request.redirect_uri)
            .append_pair("scope", SCOPES)
            .append_pair("resource", RESOURCE)
            .append_pair("state", request.state)
            .append_pair("nonce", request.nonce)
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", request.challenge);
    }
    url.into()
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: u64,
    scope: Option<String>,
    earliest_refresh_at: Option<Value>,
}

/// A token endpoint's success body. `None` when it is not one.
pub fn token_answer(body: &[u8]) -> Option<TokenAnswer> {
    let tokens: Tokens = serde_json::from_slice(body).ok()?;
    if tokens.access_token.is_empty() || tokens.expires_in == 0 {
        return None;
    }
    Some(TokenAnswer {
        access: Secret::new(tokens.access_token),
        refresh: tokens
            .refresh_token
            .filter(|token| !token.is_empty())
            .map(Secret::new),
        id_token: tokens
            .id_token
            .filter(|token| !token.is_empty())
            .map(Secret::new),
        expires_in: Duration::from_secs(tokens.expires_in),
        earliest_refresh_at_ms: tokens.earliest_refresh_at.as_ref().and_then(instant_ms),
        scopes: tokens
            .scope
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
    })
}

/// `earliest_refresh_at` as Unix milliseconds: seconds as a number, or an
/// RFC 3339 time.
fn instant_ms(value: &Value) -> Option<u64> {
    match value {
        Value::Number(seconds) => seconds.as_u64().map(|seconds| seconds.saturating_mul(1000)),
        Value::String(text) => {
            time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                .ok()
                .and_then(|at| u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok())
        }
        _ => None,
    }
}

/// The codes OpenAI's refresh errors name for a refresh token or client
/// that will not work again.
const UNUSABLE: [&str; 7] = [
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
    "invalid_client",
];

/// What a refused refresh means: unusable when OpenAI names one of
/// [`UNUSABLE`], else not reached (the tokens are kept).
pub fn refresh_failure(answer: &Answer) -> RefreshFailure {
    let code = serde_json::from_slice::<Value>(&answer.body)
        .ok()
        .and_then(|body| {
            let error = body.get("error")?;
            error
                .as_str()
                .or_else(|| error.get("code").and_then(Value::as_str))
                .map(str::to_owned)
        });
    match code {
        Some(code) if (400..500).contains(&answer.status) && UNUSABLE.contains(&code.as_str()) => {
            RefreshFailure::Unusable
        }
        _ => RefreshFailure::Unreachable,
    }
}

fn decode_part(part: &str) -> Option<Value> {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).ok()?).ok()
}

/// Who `token` names, once its RS256 signature verifies against one of
/// `keys` (a JWKS document) by its `kid`, its issuer is `issuer`, its
/// audience holds `client_id`, it has not expired at `now_ms`, and its
/// nonce is `nonce`. `None` otherwise.
pub fn verify_id_token(
    token: &str,
    keys: &Value,
    issuer: &str,
    client_id: &str,
    nonce: &str,
    now_ms: u64,
) -> Option<Identity> {
    let mut parts = token.split('.');
    let (header, claims, signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let head = decode_part(header)?;
    if head.get("alg")?.as_str()? != "RS256" {
        return None;
    }
    let kid = head.get("kid").and_then(Value::as_str);
    let key = keys.get("keys")?.as_array()?.iter().find(|key| {
        key.get("kty").and_then(Value::as_str) == Some("RSA")
            && kid.is_none_or(|kid| key.get("kid").and_then(Value::as_str) == Some(kid))
    })?;
    let n = URL_SAFE_NO_PAD.decode(key.get("n")?.as_str()?).ok()?;
    let e = URL_SAFE_NO_PAD.decode(key.get("e")?.as_str()?).ok()?;
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    ring::signature::RsaPublicKeyComponents { n: &n, e: &e }
        .verify(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            format!("{header}.{claims}").as_bytes(),
            &signature,
        )
        .ok()?;
    let claims = decode_part(claims)?;
    if claims.get("iss")?.as_str()? != issuer {
        return None;
    }
    let audience = claims.get("aud")?;
    let ours = match audience {
        Value::String(one) => one == client_id,
        Value::Array(many) => many.iter().any(|each| each.as_str() == Some(client_id)),
        _ => false,
    };
    if !ours || claims.get("nonce")?.as_str()? != nonce {
        return None;
    }
    let expires_ms = claims.get("exp")?.as_u64()?.saturating_mul(1000);
    if expires_ms.saturating_add(CLOCK_SKEW_MS) <= now_ms {
        return None;
    }
    let subject = claims.get("sub")?.as_str()?.to_owned();
    if subject.is_empty() {
        return None;
    }
    Some(Identity {
        subject,
        email: claims
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}
