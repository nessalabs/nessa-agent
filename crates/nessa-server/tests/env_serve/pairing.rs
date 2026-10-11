use super::answer_to_failure;
use crate::env_serve::application::Answer;
use nessa_auth::application::pairing::PrivateStateError;
use nessa_client_core::pairing::NativeClientError;
use std::{io::ErrorKind, path::Path};

fn unusable(answer: &Answer) -> bool {
    matches!(answer, Answer::Unusable(reason) if reason.contains("remove /data/pairing"))
}

/// Only a pairing record this machine cannot use, or a gateway answering
/// out of turn, stops waiting; everything else is asked again, a failed
/// save among them (its pending record is kept: storage failures other
/// than corrupt or conflicting land in the same arm).
#[test]
fn a_failed_status_is_unusable_only_when_no_later_ask_can_change_it() {
    let directory = Path::new("/data/pairing");
    assert_eq!(
        answer_to_failure(NativeClientError::NoPending, directory),
        Answer::NotPaired
    );
    for error in [
        NativeClientError::Storage(PrivateStateError::Corrupt),
        NativeClientError::Storage(PrivateStateError::Conflict),
        NativeClientError::Phase,
        NativeClientError::OtherEnrollee,
    ] {
        assert!(unusable(&answer_to_failure(error, directory)), "{error:?}");
    }
    for error in [
        NativeClientError::Io(ErrorKind::ConnectionReset),
        NativeClientError::Busy,
        NativeClientError::Refused,
        NativeClientError::Entropy,
        NativeClientError::Storage(PrivateStateError::Unavailable),
        NativeClientError::Storage(PrivateStateError::Uncertain),
    ] {
        assert!(
            matches!(answer_to_failure(error, directory), Answer::Unanswered(_)),
            "{error:?}"
        );
    }
}
