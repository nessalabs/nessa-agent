use crate::browser_session::domain::value_objects::BrowserSessionOrigin;

#[test]
fn browser_session_origin_preserves_structurally_valid_http_origins() {
    for value in [
        "https://historical.example:8443",
        "http://127.0.0.1:7421",
        "https://[2001:db8::1]",
        "HTTPS://Historical.Example:443",
    ] {
        let origin = BrowserSessionOrigin::new(value.to_owned()).unwrap();
        assert_eq!(origin.as_str(), value);
    }
}

#[test]
fn browser_session_origin_rejects_non_origin_and_oversized_text() {
    for value in [
        "null".to_owned(),
        "data:text/plain,opaque".to_owned(),
        "tauri://localhost".to_owned(),
        "ftp://example.com".to_owned(),
        "https://user@example.com".to_owned(),
        "https://user:password@example.com".to_owned(),
        "https://@example.com".to_owned(),
        "https://:@example.com".to_owned(),
        "https://example.com/".to_owned(),
        "https://example.com/path".to_owned(),
        "https://example.com\\".to_owned(),
        "https://example.com?query".to_owned(),
        "https://example.com#fragment".to_owned(),
        "https://example.com:".to_owned(),
        "https://example.com:65536".to_owned(),
        " https://example.com".to_owned(),
        "https://example.com\n".to_owned(),
        "https://".to_owned(),
        format!("https://{}.example", "a".repeat(1017)),
    ] {
        assert!(
            BrowserSessionOrigin::new(value.clone()).is_none(),
            "accepted {value}"
        );
    }
}
