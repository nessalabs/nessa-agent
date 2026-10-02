use super::super::McpAppError;
use super::UiResourceUri;

/// The most UTF-8 bytes of HTML a UI resource may hold: 4 MiB. An app is one
/// HTML document, usually with its script inlined.
pub const MAX_UI_HTML_BYTES: usize = 4 * 1024 * 1024;
/// The most sources one CSP list may hold.
pub const MAX_CSP_SOURCES: usize = 64;
/// The most bytes one CSP source, or the app's `domain`, may hold.
pub const MAX_CSP_SOURCE_BYTES: usize = 512;

/// The origins an app asks to reach (`_meta.ui.csp`), each a CSP source
/// such as `https://api.example.com` or `https://*.example.com`.
///
/// A source holds only ASCII letters, digits and `-._~:/*%[]@!$&()+=?#`, so it
/// can be written into a policy as one source: it cannot hold the space, `;`
/// or `,` that would end it and begin another, nor a quote that would make it
/// a keyword. Each list holds at most [`MAX_CSP_SOURCES`] sources of at most
/// [`MAX_CSP_SOURCE_BYTES`] bytes. Empty lists are what an app that asks for
/// nothing has: no network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiCsp {
    connect: Box<[Box<str>]>,
    resource: Box<[Box<str>]>,
    frame: Box<[Box<str>]>,
    base_uri: Box<[Box<str>]>,
}
impl UiCsp {
    /// The four lists the extension defines: `connectDomains` (fetch and
    /// sockets), `resourceDomains` (scripts, styles, images, fonts, media),
    /// `frameDomains` (nested frames) and `baseUriDomains` (`<base>`).
    ///
    /// # Errors
    ///
    /// [`McpAppError::TooManyValues`] for a list past [`MAX_CSP_SOURCES`],
    /// [`McpAppError::ValueTooLong`] for a source past
    /// [`MAX_CSP_SOURCE_BYTES`], and [`McpAppError::InvalidValue`] for an
    /// empty source or one outside the alphabet above.
    pub fn new(
        connect: Vec<String>,
        resource: Vec<String>,
        frame: Vec<String>,
        base_uri: Vec<String>,
    ) -> Result<Self, McpAppError> {
        Ok(Self {
            connect: sources(connect, "CSP connectDomains")?,
            resource: sources(resource, "CSP resourceDomains")?,
            frame: sources(frame, "CSP frameDomains")?,
            base_uri: sources(base_uri, "CSP baseUriDomains")?,
        })
    }
    /// Where the app may connect: fetch, XHR, WebSocket.
    pub fn connect_domains(&self) -> &[Box<str>] {
        &self.connect
    }
    /// Where the app may load scripts, styles, images, fonts and media from.
    pub fn resource_domains(&self) -> &[Box<str>] {
        &self.resource
    }
    /// What the app may frame.
    pub fn frame_domains(&self) -> &[Box<str>] {
        &self.frame
    }
    /// What the app's `<base>` may point at.
    pub fn base_uri_domains(&self) -> &[Box<str>] {
        &self.base_uri
    }
}

fn sources(values: Vec<String>, field: &'static str) -> Result<Box<[Box<str>]>, McpAppError> {
    if values.len() > MAX_CSP_SOURCES {
        return Err(McpAppError::TooManyValues {
            field,
            max: MAX_CSP_SOURCES,
        });
    }
    values
        .into_iter()
        .map(|value| source(value, field))
        .collect()
}

fn source(value: String, field: &'static str) -> Result<Box<str>, McpAppError> {
    if value.len() > MAX_CSP_SOURCE_BYTES {
        return Err(McpAppError::ValueTooLong {
            field,
            max_bytes: MAX_CSP_SOURCE_BYTES,
        });
    }
    let allowed =
        |byte: u8| byte.is_ascii_alphanumeric() || b"-._~:/*%[]@!$&()+=?#".contains(&byte);
    if value.is_empty() || !value.bytes().all(allowed) {
        return Err(McpAppError::InvalidValue(field));
    }
    Ok(value.into_boxed_str())
}

/// What an app asks the host to let it use (`_meta.ui.permissions`). Only
/// these four are known; a permission the host does not know is never
/// granted, so it is not kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiPermissions {
    /// The camera.
    pub camera: bool,
    /// The microphone.
    pub microphone: bool,
    /// The device's location.
    pub geolocation: bool,
    /// Writing to the clipboard.
    pub clipboard_write: bool,
}

/// An MCP App's UI: the HTML document a `ui://` resource holds, and what its
/// `_meta.ui` asks of the host that draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiResource {
    uri: UiResourceUri,
    html: Box<str>,
    csp: UiCsp,
    permissions: UiPermissions,
    domain: Option<Box<str>>,
    prefers_border: Option<bool>,
}
impl UiResource {
    /// The resource at `uri`: its `html`, the origins it asks for (`csp`),
    /// the `permissions` it asks for, the dedicated origin it asks to be
    /// served from (`domain`, a CSP source), and whether it would rather be
    /// drawn with a border (`prefers_border`; `None` leaves it to the host).
    ///
    /// # Errors
    ///
    /// [`McpAppError::ValueTooLong`] for HTML past [`MAX_UI_HTML_BYTES`] or a
    /// `domain` past [`MAX_CSP_SOURCE_BYTES`], and
    /// [`McpAppError::InvalidValue`] for a `domain` that could not be a CSP
    /// source.
    pub fn new(
        uri: UiResourceUri,
        html: String,
        csp: UiCsp,
        permissions: UiPermissions,
        domain: Option<String>,
        prefers_border: Option<bool>,
    ) -> Result<Self, McpAppError> {
        if html.len() > MAX_UI_HTML_BYTES {
            return Err(McpAppError::ValueTooLong {
                field: "UI resource HTML",
                max_bytes: MAX_UI_HTML_BYTES,
            });
        }
        Ok(Self {
            uri,
            html: html.into_boxed_str(),
            csp,
            permissions,
            domain: domain
                .map(|domain| source(domain, "UI domain"))
                .transpose()?,
            prefers_border,
        })
    }
    /// Where the resource was read from.
    pub fn uri(&self) -> &UiResourceUri {
        &self.uri
    }
    /// The app's HTML document.
    pub fn html(&self) -> &str {
        &self.html
    }
    /// The origins the app asks to reach.
    pub fn csp(&self) -> &UiCsp {
        &self.csp
    }
    /// What the app asks to use.
    pub fn permissions(&self) -> UiPermissions {
        self.permissions
    }
    /// The dedicated origin the app asks to be served from, if any.
    pub fn domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }
    /// Whether the app would rather be drawn with a border; `None` when it
    /// did not say.
    pub fn prefers_border(&self) -> Option<bool> {
        self.prefers_border
    }
}
