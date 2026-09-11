use nessa_sdk::domain::common::value_objects::{Date, Url};

#[test]
fn dates_preserve_precision_and_delegate_calendar_validation() {
    let month = Date::new("2026-06".into()).unwrap();
    assert_eq!(month.as_str(), "2026-06");
    assert!(!month.is_day());
    assert!(Date::new("2000-02-29".into()).unwrap().is_day());
    for invalid in [
        "1900-02-29",
        "2026-04-31",
        "2026-13",
        "2026-6",
        "2026-06-1",
        "0000-01",
        "2026-06-01T00:00:00Z",
        "ééééé",
    ] {
        assert!(Date::new(invalid.into()).is_err(), "{invalid}");
    }
}

#[test]
fn url_validation_rejects_malformed_absolute_urls_without_silently_trimming() {
    for invalid in [
        "",
        "/relative",
        "https://",
        "https://[invalid]",
        "https://example.com:99999",
        "https://exa mple.com",
        " https://example.com",
        "https://example.com\n",
    ] {
        assert!(Url::new(invalid).is_err(), "{invalid}");
        assert!(Url::validate(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn urls_are_reusable_across_schemes_and_have_canonical_value_equality() {
    assert_eq!(
        Url::new("HTTPS://EXAMPLE.COM:443/a").unwrap(),
        Url::new("https://example.com/a").unwrap()
    );
    for valid in [
        "http://example.com",
        "mailto:someone@example.com",
        "https://[::1]/",
        "https://example.com/a?q=1#part",
    ] {
        assert!(Url::validate(valid).is_ok(), "{valid}");
    }
    assert_eq!(Url::new("https://example.com").unwrap().scheme(), "https");
}
