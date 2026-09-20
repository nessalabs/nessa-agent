//! Loopback and desktop-shell origins this process trusts, shared by every
//! context that checks a browser `Origin`: request admission in the server and
//! agents entrypoints, and browser-session journal validation.

/// Accept the desktop shell origin or a loopback `http(s)` origin with an
/// optional numeric port. Anything else, including userinfo tricks and paths,
/// is rejected.
pub fn is_trusted_origin_value(origin: &str) -> bool {
    if origin == "tauri://localhost" {
        return true;
    }
    let Some(authority) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|host| {
        authority == *host
            || authority
                .strip_prefix(host)
                .and_then(|suffix| suffix.strip_prefix(':'))
                .is_some_and(|port| {
                    !port.is_empty()
                        && port.bytes().all(|byte| byte.is_ascii_digit())
                        && port.parse::<u16>().is_ok()
                })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_untrusted_browser_origin() {
        for origin in [
            "https://evil.example",
            "http://localhost:80@evil.example",
            "http://127.0.0.1:5173/evil",
            "http://localhost:",
            "http://localhost:65536",
            "http://localhost:+80",
            "null",
        ] {
            assert!(!is_trusted_origin_value(origin), "accepted {origin}");
        }
    }

    #[test]
    fn accepts_loopback_browser_origin() {
        assert!(is_trusted_origin_value("http://127.0.0.1:5173"));
        assert!(is_trusted_origin_value("https://localhost"));
        assert!(is_trusted_origin_value("http://[::1]:5173"));
        assert!(is_trusted_origin_value("tauri://localhost"));
    }
}
