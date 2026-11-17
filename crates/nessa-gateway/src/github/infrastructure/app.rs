//! Which GitHub App this gateway signs in through, and where GitHub is.
//!
//! The released app is `protocol/defaults/github-app.json`: its client id
//! and slug are public, and no client secret exists anywhere, because the
//! device flow needs none and the app's user tokens do not expire.
//! Composition may read another file in its place (`NESSA_GITHUB_APP`) to
//! point a test at a fake GitHub; what such a file may say is decided here.
use serde::Deserialize;
use url::Url;

const RELEASED: &str = include_str!("../../../../../protocol/defaults/github-app.json");

/// A GitHub App's public facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubApp {
    pub client_id: String,
    pub slug: String,
    /// github.com, where codes are asked for and approved.
    pub web_url: Url,
    /// api.github.com, where the signed-in account is read.
    pub api_url: Url,
}

/// Why a description of the app was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GithubAppProblem {
    /// Not the JSON shape, or a field missing.
    Malformed,
    /// An empty or unusable client id or slug.
    Identity,
    /// A URL that is not https, or http to this machine, or that carries a
    /// user, a query or a fragment.
    Url,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Described {
    client_id: String,
    slug: String,
    web_url: String,
    api_url: String,
}

impl GithubApp {
    /// The app the release signs in through.
    pub fn released() -> Self {
        Self::parse(RELEASED.as_bytes()).expect("protocol/defaults/github-app.json is valid")
    }

    /// The app `bytes` describe.
    pub fn parse(bytes: &[u8]) -> Result<Self, GithubAppProblem> {
        let described: Described =
            serde_json::from_slice(bytes).map_err(|_| GithubAppProblem::Malformed)?;
        let plain = |value: &str, also: &[u8]| {
            !value.is_empty()
                && value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || byte == b'-' || also.contains(&byte)
                })
        };
        // Older apps' client ids read `Iv1.0123abcd`; a slug is a URL segment.
        if !plain(&described.client_id, b"._") || !plain(&described.slug, b"_") {
            return Err(GithubAppProblem::Identity);
        }
        Ok(Self {
            client_id: described.client_id,
            slug: described.slug,
            web_url: base(&described.web_url)?,
            api_url: base(&described.api_url)?,
        })
    }

    /// Where the person picks the repositories the app may use.
    pub fn install_url(&self) -> String {
        self.at(
            &self.web_url,
            &format!("apps/{}/installations/new", self.slug),
        )
    }

    /// `path` beneath `base`, which always ends in `/`.
    pub(super) fn at(&self, base: &Url, path: &str) -> String {
        format!("{}{path}", base.as_str())
    }
}

/// A base URL, ending in `/`: https anywhere, or http only to this machine.
fn base(value: &str) -> Result<Url, GithubAppProblem> {
    let mut url = Url::parse(value).map_err(|_| GithubAppProblem::Url)?;
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
        return Err(GithubAppProblem::Url);
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn described(web: &str, api: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "clientId": "Iv23abc", "slug": "nessa-agent", "webUrl": web, "apiUrl": api,
        }))
        .unwrap()
    }

    #[test]
    fn the_released_app_is_nessas_on_github() {
        let app = GithubApp::released();
        assert_eq!(app.slug, "nessa-agent");
        assert_eq!(app.web_url.as_str(), "https://github.com/");
        assert_eq!(app.api_url.as_str(), "https://api.github.com/");
        assert_eq!(
            app.install_url(),
            "https://github.com/apps/nessa-agent/installations/new"
        );
    }

    #[test]
    fn http_is_only_for_this_machine() {
        let app =
            GithubApp::parse(&described("http://127.0.0.1:9/", "http://localhost:9/api")).unwrap();
        assert_eq!(app.api_url.as_str(), "http://localhost:9/api/");
        for (web, api) in [
            ("http://example.com/", "https://api.github.com/"),
            ("https://github.com/?q", "https://api.github.com/"),
            ("https://user@github.com/", "https://api.github.com/"),
            ("https://github.com/#x", "https://api.github.com/"),
            ("ftp://github.com/", "https://api.github.com/"),
        ] {
            assert_eq!(
                GithubApp::parse(&described(web, api)),
                Err(GithubAppProblem::Url),
                "{web}"
            );
        }
    }

    #[test]
    fn an_identity_is_plain_and_present() {
        for (client, slug) in [
            ("", "nessa"),
            ("Iv2", ""),
            ("Iv2 x", "nessa"),
            ("Iv2", "a/b"),
            ("Iv2", "a.b"),
        ] {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "clientId": client, "slug": slug,
                "webUrl": "https://github.com", "apiUrl": "https://api.github.com",
            }))
            .unwrap();
            assert_eq!(GithubApp::parse(&bytes), Err(GithubAppProblem::Identity));
        }
        assert_eq!(GithubApp::parse(b"{}"), Err(GithubAppProblem::Malformed));
        let older = serde_json::to_vec(&serde_json::json!({
            "clientId": "Iv1.0123abcd", "slug": "nessa-agent",
            "webUrl": "https://github.com", "apiUrl": "https://api.github.com",
        }))
        .unwrap();
        assert!(GithubApp::parse(&older).is_ok(), "an older app's client id");
    }
}
