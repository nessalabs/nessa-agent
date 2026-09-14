use nessa_sdk::domain::common::value_objects::{Date, TokenLimits, TokenLimitsError, Url};

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

#[test]
fn date_and_url_diagnostics_preserve_actionable_parse_errors() {
    let date = Date::new("2026-02-30".into()).unwrap_err();
    assert_eq!(
        date.to_string(),
        "expected a valid calendar date in YYYY-MM or YYYY-MM-DD format"
    );
    assert!(std::error::Error::source(&date).is_none());
    let whitespace = Url::new("https://example.com/has space").unwrap_err();
    assert_eq!(
        whitespace.to_string(),
        "URL must not contain whitespace or control characters"
    );
    assert!(std::error::Error::source(&whitespace).is_none());
    let invalid = Url::new("relative/path").unwrap_err();
    let source =
        std::error::Error::source(&invalid).expect("URL parser errors preserve their source");
    assert_eq!(source.to_string(), "relative URL without a base");
    assert_eq!(
        invalid.to_string(),
        "invalid absolute URL: relative URL without a base"
    );
}

#[test]
fn token_limits_cannot_be_zero_or_exceed_the_context_window() {
    for (context, output, expected) in [
        (0, 0, TokenLimitsError::ZeroContextWindow),
        (0, 1, TokenLimitsError::ZeroContextWindow),
        (100, 0, TokenLimitsError::ZeroOutput),
        (
            100,
            101,
            TokenLimitsError::OutputExceedsContext {
                context_window: 100,
                output: 101,
            },
        ),
    ] {
        assert_eq!(TokenLimits::new(context, output), Err(expected));
    }
    for (context, output) in [(1, 1), (100, 100), (u32::MAX, u32::MAX)] {
        let limits = TokenLimits::new(context, output).unwrap();
        assert_eq!(limits.max_context_window(), context);
        assert_eq!(limits.max_output(), output);
    }
}

#[test]
fn context_usage_uses_the_instances_window() {
    let limits = TokenLimits::new(200_000, 128_000).unwrap();
    for (used, percentage) in [(0, 0.0), (50_000, 25.0), (200_000, 100.0), (250_000, 125.0)] {
        assert_eq!(limits.context_usage_percent(used), percentage);
    }
}

#[test]
fn token_limit_diagnostics_describe_the_invalid_shared_value() {
    for (context, output, message) in [
        (0, 1, "maximum context window: must be positive"),
        (100, 0, "maximum output: must be positive"),
        (
            100,
            101,
            "maximum output 101 exceeds maximum context window 100",
        ),
    ] {
        let error = TokenLimits::new(context, output).unwrap_err();
        assert_eq!(error.to_string(), message);
        assert!(std::error::Error::source(&error).is_none());
    }
}
