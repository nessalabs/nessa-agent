use super::*;

#[test]
fn destinations_are_constrained_and_paths_are_redacted() {
    for bad in [
        "",
        "http://recipient.test/",
        "http://recipient.test:443/",
        "ftp://recipient.test/",
        "https://127.0.0.1/",
        "https://[::1]/",
        "https://2130706433/",
        "https://user:pass@recipient.test/",
        "https://user@recipient.test/",
        "https://recipient.test:8443/",
        "https://recipient.test/?key=secret",
        "https://recipient.test/#secret",
        "https://recipient.test/\r\nX:secret",
    ] {
        assert!(Destination::parse(bad).is_err(), "accepted {bad:?}");
    }
    assert!(Destination::parse(&format!("https://recipient.test/{}", "x".repeat(2048))).is_err());
    let destination = Destination::parse("https://recipient.test/private-path-secret").unwrap();
    assert_eq!(destination.expose_url().port_or_known_default(), Some(443));
    assert_eq!(format!("{destination:?}"), "Destination([REDACTED])");
}
