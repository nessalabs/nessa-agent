//! The foreign adapter preserves absence/failure and delegates actual disk reads.
use super::*;
use crate::test_support::PrivateFixture;
const CONVERSATION: &str = "00000000-0000-4000-8000-000000000001";
#[test]
fn foreign_snapshot_absence_survives_cache_recreation() {
    let root = PrivateFixture::new();
    let path = root
        .path()
        .join("cache.sqlite")
        .to_str()
        .unwrap()
        .to_owned();
    for _ in 0..2 {
        let cache = NativeSavedCache::open(path.clone()).unwrap();
        let snapshot = cache
            .transcript(
                "receiver".into(),
                "origin".into(),
                CONVERSATION.into(),
                CONVERSATION.into(),
            )
            .unwrap();
        drop(cache);
        assert_eq!(snapshot.state(), NativeTranscriptState::NotLoaded);
        assert_eq!(snapshot.downloaded(), None);
        assert_eq!(snapshot.applied(), None);
        assert_eq!(snapshot.facts(), None);
        assert_eq!(snapshot.generation(), None);
        assert_eq!(snapshot.view_json().unwrap(), None);
    }
}
#[test]
fn foreign_invalid_selection_is_a_failure_not_absence() {
    let root = PrivateFixture::new();
    let cache =
        NativeSavedCache::open(root.path().join("cache.sqlite").to_str().unwrap().into()).unwrap();
    assert!(matches!(
        cache.transcript(
            "".into(),
            "origin".into(),
            CONVERSATION.into(),
            CONVERSATION.into()
        ),
        Err(NativeViewError::InvalidTarget)
    ));
}
#[test]
fn foreign_direct_concurrent_call_refuses_without_waiting() {
    let root = PrivateFixture::new();
    let cache =
        NativeSavedCache::open(root.path().join("cache.sqlite").to_str().unwrap().into()).unwrap();
    let _admitted = cache.cache.lock().unwrap();
    assert!(matches!(
        cache.transcript(
            "receiver".into(),
            "origin".into(),
            CONVERSATION.into(),
            CONVERSATION.into()
        ),
        Err(NativeViewError::Busy)
    ));
}
#[test]
fn serialization_bound_does_not_return_partial_view() {
    let valid = serde_json::json!({"text":"short"});
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&serialize_view(&valid).unwrap()).unwrap(),
        valid
    );
    let oversized = serde_json::json!({"text":"a".repeat(MAX_VIEW_BYTES)});
    assert_eq!(
        serialize_view(&oversized),
        Err(NativeViewError::ViewTooLarge)
    );
    assert!(
        serialize_view(&valid).is_ok(),
        "a failure cannot affect a later serialization"
    );
}

#[test]
fn foreign_deletion_marker_is_not_missing_data() {
    let snapshot = NativeTranscript {
        value: RetainedTranscript::Deleted,
    };
    assert_eq!(snapshot.state(), NativeTranscriptState::Deleted);
    assert_eq!(snapshot.downloaded(), None);
    assert_eq!(snapshot.view_json().unwrap(), None);
}
