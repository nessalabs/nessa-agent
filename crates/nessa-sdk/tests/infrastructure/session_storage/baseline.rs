//! A validated JSONL session becomes one sealed, inert baseline candidate.
use super::*;
use nessa_sdk::infrastructure::session_storage::{
    decode_baseline, encode_baseline, BaselineSection,
};

#[tokio::test]
async fn saved_session_baseline_round_trip_preserves_input_observation_and_receipt() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let original = snapshot("baseline-round-trip");
    let lease = storage.open(original.id.clone()).await.unwrap();
    lease.save(original.clone()).await.unwrap();
    let saved = SessionSnapshot::load_saved(&*lease, &original.id)
        .await
        .unwrap()
        .unwrap();
    let export = encode_baseline(&saved).unwrap();
    assert_eq!(export.pieces[0].section, BaselineSection::Header);
    assert_eq!(export.pieces[1].section, BaselineSection::Queue);
    assert_eq!(export.pieces[2].section, BaselineSection::Invocation(0));
    let recovered = decode_baseline(&export).unwrap();
    assert_same(&recovered, &original);
    assert_eq!(
        recovered.invocations[0].acknowledgement,
        original.invocations[0].acknowledgement
    );
    assert_eq!(encode_baseline(&recovered).unwrap(), export);
}

#[test]
fn baseline_keeps_global_queue_membership_and_each_invocation_identity() {
    let source = super::queue_history::pending(&["one", "two"]);
    let recovered = decode_baseline(&encode_baseline(&source).unwrap()).unwrap();
    assert_eq!(recovered.queue_history, source.queue_history);
    assert_eq!(recovered.invocations.len(), 2);
    for (actual, expected) in recovered.invocations.iter().zip(&source.invocations) {
        assert_eq!(actual.request, expected.request);
        assert_eq!(actual.actor, expected.actor);
        assert_eq!(actual.acknowledgement, expected.acknowledgement);
        assert_eq!(actual.scheduling, expected.scheduling);
    }
}

#[test]
fn missing_changed_or_reordered_pieces_cannot_expose_a_baseline() {
    let mut source = snapshot("bounded-baseline");
    source.invocations[0].request.user_message =
        UserMessage::text_only(PromptText::new("x".repeat(2 * 64 * 1024)).unwrap());
    let export = encode_baseline(&source).unwrap();
    assert!(export.pieces.len() > 4);
    assert!(export
        .pieces
        .iter()
        .all(|piece| !piece.bytes.is_empty() && piece.bytes.len() <= 64 * 1024));
    assert_same(&decode_baseline(&export).unwrap(), &source);

    let mut missing = export.clone();
    missing.pieces.remove(3);
    assert!(matches!(
        decode_baseline(&missing),
        Err(StorageError::Corrupt(_))
    ));

    let mut changed = export.clone();
    changed.pieces[3].bytes[0] ^= 1;
    assert!(matches!(
        decode_baseline(&changed),
        Err(StorageError::Corrupt(_))
    ));

    let mut reordered = export.clone();
    reordered.pieces.swap(2, 3);
    assert!(matches!(
        decode_baseline(&reordered),
        Err(StorageError::Corrupt(_))
    ));
}
