use super::*;
use crate::webhooks::domain::Observation;
use uuid::Uuid;

#[test]
fn status_and_failure_classification_preserve_ambiguity() {
    for status in [200, 201, 204, 299] {
        assert_eq!(observation(Ok(status)), Observation::Success);
    }
    for status in [408, 429, 500, 503, 599] {
        assert_eq!(observation(Ok(status)), Observation::Retryable);
    }
    for status in [100, 199, 300, 301, 307, 308, 400, 401, 404, 409, 499] {
        assert_eq!(observation(Ok(status)), Observation::Rejected);
    }
    assert_eq!(
        observation(Err(PostFailure::NotSent)),
        Observation::Retryable
    );
    for failure in [
        PostFailure::IncompleteResponse,
        PostFailure::ResponseTooLarge,
    ] {
        assert_eq!(observation(Err(failure)), Observation::Unknown);
    }
    assert_eq!(
        observation(Err(PostFailure::TimedOut)),
        Observation::TimedOut
    );
}

#[test]
fn application_values_bound_retained_bytes_and_redact_diagnostics() {
    let key = MacKey::new([9; 32]);
    assert_eq!(format!("{key:?}"), "MacKey([REDACTED])");
    let message = Message::encoded(Uuid::nil(), 1, b"private bytes".to_vec()).unwrap();
    assert_eq!(format!("{message:?}"), "Message([REDACTED])");
    assert!(matches!(
        Message::encoded(Uuid::nil(), 1, vec![0; MAX_BODY_BYTES + 1]),
        Err(InvalidMessage)
    ));
}
