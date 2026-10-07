//! Protected-resource and authorization-server discovery. Every
//! authorization-server URL must be HTTPS. The resource probe may be the
//! configured MCP URL, including loopback HTTP; its bearer is never sent.
use crate::mcp_authorization::application::ports::{OAuthCallFailure, OAuthHttp, OAuthResponse};
use serde_json::Value;
use std::sync::Arc;

/// Metadata the chart will accept or refuse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovered {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub revocation_endpoint: Option<String>,
    pub pkce_s256: bool,
    pub scopes: Vec<String>,
    pub resource_matches: bool,
    pub issuer_matches: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoverFailure {
    Unreachable,
    UnsupportedRegistration,
    Binding,
    Malformed,
}

/// The challenge's `scope` parameter, which is authoritative when present.
pub fn challenge_scope(www_authenticate: &str) -> Option<String> {
    parameter(www_authenticate, "scope")
}

/// `WWW-Authenticate`'s `resource_metadata` parameter, when the challenge
/// is a Bearer challenge that carries one.
pub fn resource_metadata(www_authenticate: &str) -> Option<String> {
    parameter(www_authenticate, "resource_metadata")
}

fn parameter(www_authenticate: &str, name: &str) -> Option<String> {
    let rest = www_authenticate.trim().strip_prefix("Bearer")?;
    for part in rest.split(',') {
        let mut sides = part.trim().splitn(2, '=');
        if sides.next()?.trim() != name {
            continue;
        }
        let raw = sides.next()?.trim();
        let value = raw
            .strip_prefix('"')
            .and_then(|body| body.strip_suffix('"'))
            .unwrap_or(raw);
        if value.is_empty() {
            return None;
        }
        return Some(value.to_owned());
    }
    None
}

/// Path-specific then root protected-resource metadata URLs for `resource`.
pub fn protected_resource_urls(resource: &str) -> Vec<String> {
    let Ok(url) = url::Url::parse(resource) else {
        return Vec::new();
    };
    let origin = match url.origin() {
        url::Origin::Tuple(scheme, host, port) => format!("{scheme}://{host}:{port}"),
        _ => return Vec::new(),
    };
    let path = url.path().trim_end_matches('/');
    let mut urls = Vec::new();
    if !path.is_empty() && path != "/" {
        urls.push(format!(
            "{origin}/.well-known/oauth-protected-resource{path}"
        ));
    }
    urls.push(format!("{origin}/.well-known/oauth-protected-resource"));
    urls
}

pub fn authorization_server_urls(issuer: &str) -> Vec<String> {
    let issuer = issuer.trim_end_matches('/');
    vec![
        format!("{issuer}/.well-known/oauth-authorization-server"),
        format!("{issuer}/.well-known/openid-configuration"),
    ]
}

pub fn https_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}

/// Read protected-resource metadata, then the authorization server's
/// OAuth metadata or its OpenID configuration.
pub async fn discover(
    http: &Arc<dyn OAuthHttp>,
    resource: &str,
    challenge: Option<&str>,
) -> Result<Discovered, DiscoverFailure> {
    let metadata_url = match challenge.and_then(resource_metadata) {
        Some(url) if https_url(&url) => url,
        Some(_) => return Err(DiscoverFailure::Binding),
        None => {
            let mut found = None;
            for candidate in protected_resource_urls(resource) {
                if !https_url(&candidate) {
                    continue;
                }
                match http.get(&candidate).await {
                    Ok(response) if response.status == 200 => {
                        found = Some((candidate, response.body));
                        break;
                    }
                    Ok(_) => continue,
                    Err(OAuthCallFailure::NotSent) => continue,
                    Err(OAuthCallFailure::Lost) => return Err(DiscoverFailure::Unreachable),
                }
            }
            let Some((url, body)) = found else {
                return Err(DiscoverFailure::Malformed);
            };
            return finish(http, resource, &url, &body).await;
        }
    };
    let response = http
        .get(&metadata_url)
        .await
        .map_err(|_| DiscoverFailure::Unreachable)?;
    if response.status != 200 {
        return Err(DiscoverFailure::Malformed);
    }
    finish(http, resource, &metadata_url, &response.body).await
}

async fn finish(
    http: &Arc<dyn OAuthHttp>,
    resource: &str,
    metadata_url: &str,
    body: &str,
) -> Result<Discovered, DiscoverFailure> {
    let _ = metadata_url;
    let document: Value = serde_json::from_str(body).map_err(|_| DiscoverFailure::Malformed)?;
    let declared = text(&document, "resource").ok_or(DiscoverFailure::Malformed)?;
    let resource_matches = declared == resource;
    let issuer = document
        .get("authorization_servers")
        .and_then(Value::as_array)
        .and_then(|servers| servers.first())
        .and_then(Value::as_str)
        .ok_or(DiscoverFailure::Malformed)?;
    if !https_url(issuer) {
        return Err(DiscoverFailure::Binding);
    }
    let scopes = strings(document.get("scopes_supported"));
    let mut last = DiscoverFailure::Malformed;
    for candidate in authorization_server_urls(issuer) {
        if !https_url(&candidate) {
            last = DiscoverFailure::Binding;
            continue;
        }
        match http.get(&candidate).await {
            Ok(response) if response.status == 200 => {
                return read_server(resource, resource_matches, issuer, scopes, &response);
            }
            Ok(_) => last = DiscoverFailure::Malformed,
            Err(OAuthCallFailure::Lost) => return Err(DiscoverFailure::Unreachable),
            Err(OAuthCallFailure::NotSent) => last = DiscoverFailure::Unreachable,
        }
    }
    Err(last)
}

fn read_server(
    resource: &str,
    resource_matches: bool,
    issuer: &str,
    scopes: Vec<String>,
    response: &OAuthResponse,
) -> Result<Discovered, DiscoverFailure> {
    let _ = resource;
    let document: Value =
        serde_json::from_str(&response.body).map_err(|_| DiscoverFailure::Malformed)?;
    let declared = text(&document, "issuer").ok_or(DiscoverFailure::Malformed)?;
    let authorization_endpoint =
        text(&document, "authorization_endpoint").ok_or(DiscoverFailure::Malformed)?;
    let token_endpoint = text(&document, "token_endpoint").ok_or(DiscoverFailure::Malformed)?;
    for endpoint in [
        authorization_endpoint.as_str(),
        token_endpoint.as_str(),
        declared.as_str(),
    ] {
        if !https_url(endpoint) {
            return Err(DiscoverFailure::Binding);
        }
    }
    let registration = optional_https(&document, "registration_endpoint")?;
    let revocation = optional_https(&document, "revocation_endpoint")?;
    let methods = strings(document.get("code_challenge_methods_supported"));
    let pkce_s256 = methods.iter().any(|method| method == "S256");
    if registration.is_none() {
        return Err(DiscoverFailure::UnsupportedRegistration);
    }
    Ok(Discovered {
        issuer: declared.clone(),
        authorization_endpoint,
        token_endpoint,
        registration_endpoint: registration,
        revocation_endpoint: revocation,
        pkce_s256,
        scopes,
        resource_matches,
        issuer_matches: declared.trim_end_matches('/') == issuer.trim_end_matches('/'),
    })
}

fn optional_https(document: &Value, key: &str) -> Result<Option<String>, DiscoverFailure> {
    match document.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if https_url(value) => Ok(Some(value.clone())),
        Some(_) => Err(DiscoverFailure::Binding),
    }
}

fn text(document: &Value, key: &str) -> Option<String> {
    document.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// S256 challenge of `verifier`.
pub fn s256(verifier: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{}={}", urlencoding_encode(name), urlencoding_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencoding_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
