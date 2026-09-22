use url::Url;

const MAX_BROWSER_SESSION_ORIGIN_BYTES: usize = 1024;

/// The exact HTTP(S) origin text bound to a browser session.
///
/// Construction validates the origin structurally but retains the caller's
/// original serialization. Whether a deployment currently trusts that origin
/// is a separate admission decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserSessionOrigin(String);

impl BrowserSessionOrigin {
    /// Validate and retain one browser-session origin.
    pub fn new(value: String) -> Option<Self> {
        if value.len() > MAX_BROWSER_SESSION_ORIGIN_BYTES
            || value
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            return None;
        }
        let parsed = Url::parse(&value).ok()?;
        let authority = value.split_once("://")?.1;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.cannot_be_a_base()
            || parsed.host().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
            || authority.ends_with(':')
            || authority
                .bytes()
                .any(|byte| matches!(byte, b'/' | b'?' | b'#'))
        {
            return None;
        }
        Some(Self(value))
    }

    /// Return the preserved origin serialization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for BrowserSessionOrigin {
    type Error = ();

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/browser_session/browser_session_origin.rs"]
mod tests;
