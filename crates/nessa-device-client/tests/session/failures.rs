//! Foreign immutable failure views retain the original typed payload meaning.
use super::*;
use nessa_protocol::product_contract::generated::ConversationErrorCode;
#[test]
fn private_acknowledgement_uncertainty_is_not_no_publication() {
    let failure = NativeEnrollmentFailure {
        value: NativeClientError::Storage(PrivateStateError::Uncertain),
    };
    assert_eq!(failure.kind(), NativeEnrollmentFailureKind::Storage);
    let storage = failure.storage().unwrap();
    assert_eq!(storage.kind(), NativePrivateStateKind::Uncertain);
    assert!(storage.publication().is_none());
    assert!(storage.key_publication().is_none());
    assert!(failure.crypto().is_none() && failure.wire().is_none());
    let audit = NativePrivateStateFailure {
        value: PrivateStateError::AuditUnavailable(GatewayPublicationState::Published),
    };
    assert_eq!(audit.kind(), NativePrivateStateKind::AuditUnavailable);
    assert_eq!(
        audit.key_publication(),
        Some(NativeKeyPublication::Published)
    );
    assert!(audit.publication().is_none());
}
#[test]
fn handshake_and_crypto_and_wire_correlation_remain_distinct() {
    let handshake = NativeEnrollmentFailure {
        value: NativeClientError::Handshake(PairingCryptoError::InvalidProof),
    };
    let crypto = NativeEnrollmentFailure {
        value: NativeClientError::Crypto(PairingCryptoError::InvalidProof),
    };
    assert_eq!(handshake.kind(), NativeEnrollmentFailureKind::Handshake);
    assert_eq!(crypto.kind(), NativeEnrollmentFailureKind::Crypto);
    assert_eq!(handshake.crypto(), Some(NativeCryptoFailure::InvalidProof));
    assert_eq!(crypto.crypto(), Some(NativeCryptoFailure::InvalidProof));
    let wire = NativeEnrollmentFailure {
        value: NativeClientError::Wire(NativeWireError::Correlation(PairingError::Conflict)),
    };
    assert_eq!(wire.kind(), NativeEnrollmentFailureKind::Wire);
    assert_eq!(
        wire.wire().unwrap().kind(),
        NativeWireFailureKind::Correlation
    );
    assert_eq!(
        wire.wire().unwrap().correlation(),
        Some(NativePairingFailure::Conflict)
    );
}
#[test]
fn session_failure_keeps_its_exact_original_payload() {
    let failure = NativeSessionFailure {
        value: SessionFailure::Enrollment(NativeClientError::WorkerFault(
            PairingWorkerFault::Panic,
        )),
    };
    assert_eq!(failure.kind(), NativeSessionFailureKind::Enrollment);
    assert_eq!(
        failure.enrollment().unwrap().worker(),
        Some(NativeWorkerFault::Panic)
    );
    assert!(failure.cache().is_none() && failure.private_state().is_none());
    let closed = NativeSessionFailure {
        value: SessionFailure::Closed,
    };
    assert_eq!(closed.kind(), NativeSessionFailureKind::Closed);
    assert!(closed.enrollment().is_none() && closed.cache().is_none());
    let cache = NativeSessionFailure {
        value: SessionFailure::Cache(ReadFailure::ResetRequired),
    };
    assert_eq!(cache.kind(), NativeSessionFailureKind::Cache);
    assert_eq!(cache.cache(), Some(NativeReadFailure::ResetRequired));
}

#[test]
fn cursor_ahead_stays_distinct_from_an_ungranted_conversation() {
    assert_eq!(
        NativeReadFailure::from(ReadFailure::CursorAhead),
        NativeReadFailure::CursorAhead
    );
    assert_ne!(
        NativeReadFailure::from(ReadFailure::CursorAhead),
        NativeReadFailure::Refused
    );
    assert_eq!(
        NativeReadFailure::from(ReadFailure::SubscriptionCapacity),
        NativeReadFailure::SubscriptionCapacity
    );
    assert_ne!(
        NativeReadFailure::from(ReadFailure::SubscriptionCapacity),
        NativeReadFailure::Refused
    );
    let released = NativeReadFailure::from(ReadFailure::Product(
        ConversationErrorCode::AttachmentNotFound,
    ));
    let unavailable = NativeReadFailure::from(ReadFailure::Product(
        ConversationErrorCode::AttachmentUnavailable,
    ));
    let unconfigured = NativeReadFailure::from(ReadFailure::Product(
        ConversationErrorCode::ConversationsNotConfigured,
    ));
    assert_eq!(
        released,
        NativeReadFailure::Product {
            code: "attachment_not_found".to_owned()
        }
    );
    assert_eq!(
        unavailable,
        NativeReadFailure::Product {
            code: "attachment_unavailable".to_owned()
        }
    );
    assert_eq!(
        unconfigured,
        NativeReadFailure::Product {
            code: "conversations_not_configured".to_owned()
        }
    );
    assert_ne!(released, unavailable);
    assert_ne!(released, NativeReadFailure::Protocol);
    assert_ne!(unavailable, NativeReadFailure::Refused);
    assert_ne!(unconfigured, NativeReadFailure::Protocol);
}

#[test]
fn uncertain_cache_commit_survives_foreign_failure_translation() {
    let failure = NativeSessionFailure {
        value: SessionFailure::Cache(ReadFailure::CacheUncertain),
    };
    assert_eq!(failure.cache(), Some(NativeReadFailure::CacheUncertain));
    assert_ne!(failure.cache(), Some(NativeReadFailure::Cache));
}
