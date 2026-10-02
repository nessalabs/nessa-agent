//! Exact held-lifetime manifests and durable retirement/reupload boundaries.
use super::*;
use crate::attachments::application::{ArtifactReadError, ArtifactState, AttachmentArtifacts};
use nessa_auth::domain::MAX_IDENTIFIER_BYTES;
use serde_json::Value;

#[tokio::test]
async fn manifests_keep_the_exact_stored_lifetime_across_restart_and_reupload() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"original image", b"normalized image");
    let old = claim(&store, &hold, b"normalized image").await;
    let id = ArtifactId::from_generation(old.as_str());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        store.confirm(&hold, &old).await,
        Ok(Confirmation::Confirmed)
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Live(hold.stored().clone()))
    );
    assert!(matches!(
        keep(&store, &hold, b"normalized image").await,
        Kept::Existing(_)
    ));
    drop(store);
    let store = open(root.path());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Live(hold.stored().clone()))
    );
    store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    let retired = fs::read(root.path().join("attachments").join(path_of(&hold))).unwrap();
    assert_eq!(
        store
            .discard(&hold, &old, RevertCause::ConfirmationFailed)
            .await,
        Ok(Discard::NotMine)
    );
    // Private cleanup also cannot erase a retirement or change its evidence.
    store.files.discard_generation(&hold, old.as_str());
    assert_eq!(
        fs::read(root.path().join("attachments").join(path_of(&hold))).unwrap(),
        retired
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
    let new = claim(&store, &hold, b"normalized image").await;
    assert_ne!(new, old);
    assert_eq!(store.confirm(&hold, &old).await, Ok(Confirmation::Gone));
    assert_eq!(
        store
            .discard(&hold, &old, RevertCause::AuditUnconfirmed)
            .await,
        Ok(Discard::NotMine)
    );
    assert_eq!(
        store.confirm(&hold, &new).await,
        Ok(Confirmation::Confirmed)
    );
    let new_id = ArtifactId::from_generation(new.as_str());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &new_id)
            .await
            .unwrap(),
        Some(ArtifactState::Live(hold.stored().clone()))
    );
    drop(store);
    let store = open(root.path());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &new_id)
            .await
            .unwrap(),
        Some(ArtifactState::Live(hold.stored().clone()))
    );
    let archived = fs::read(
        root.path()
            .join("attachments")
            .join(archive_path(&hold, old.as_str())),
    )
    .unwrap();
    assert_eq!(archived, retired);
}

#[tokio::test]
async fn take_back_retains_actual_revert_evidence_even_after_a_live_manifest() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"result", b"result");
    let claim = claim(&store, &hold, b"result").await;
    let id = ArtifactId::from_generation(claim.as_str());
    store.confirm(&hold, &claim).await.unwrap();
    assert!(matches!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Live(_))
    ));
    assert_eq!(
        store
            .discard(&hold, &claim, RevertCause::ConfirmationFailed)
            .await,
        Ok(Discard::Discarded {
            was: RetiredFrom::Held
        })
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
    let record =
        decode(&fs::read(root.path().join("attachments").join(path_of(&hold))).unwrap()).unwrap();
    assert_eq!(
        record.state,
        RecordState::Retired {
            was: RetiredFrom::Held,
            evidence: RetirementEvidence::RevertedUpload {
                cause: RevertCause::ConfirmationFailed,
                caller: hold.uploaded_by().clone()
            },
        }
    );
    assert_eq!(store.confirm(&hold, &claim).await, Ok(Confirmation::Gone));
    assert_eq!(
        store
            .discard(&hold, &claim, RevertCause::AuditUnconfirmed)
            .await,
        Ok(Discard::NotMine)
    );
}

#[tokio::test]
async fn a_tombstone_for_one_conversation_never_grants_shared_bytes_to_another() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let first = hold_for("org", CONVERSATION, b"shared", b"shared");
    let second = hold_for("org", OTHER_CONVERSATION, b"shared", b"shared");
    let first_claim = claim(&store, &first, b"shared").await;
    let second_claim = claim(&store, &second, b"shared").await;
    store.confirm(&first, &first_claim).await.unwrap();
    store.confirm(&second, &second_claim).await.unwrap();
    let id = ArtifactId::from_generation(first_claim.as_str());
    store
        .release(
            first.organization_id(),
            first.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    assert!(holds(&store, &second).await);
    assert_eq!(
        store
            .manifest(first.organization_id(), first.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
    assert_eq!(
        store
            .manifest(second.organization_id(), second.conversation_id(), &id)
            .await
            .unwrap(),
        None
    );
    assert!(store
        .read(digest_of(b"shared"), 16)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn a_conflicting_retirement_archive_blocks_reupload_without_replacing_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let old = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &old).await.unwrap();
    store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    let primary = root.path().join("attachments").join(path_of(&hold));
    let original = fs::read(&primary).unwrap();
    let mut conflict: Value = serde_json::from_slice(&original).unwrap();
    conflict["retirement"]["evidence"]["requestedAtMs"] = serde_json::json!(9_999);
    let archive = root
        .path()
        .join("attachments")
        .join(archive_path(&hold, old.as_str()));
    let relative = archive_path(&hold, old.as_str());
    let mut copied =
        PrivateTempFile::new_beneath(&store.files.root, relative.parent().unwrap()).unwrap();
    copied
        .as_file_mut()
        .write_all(&serde_json::to_vec(&conflict).unwrap())
        .unwrap();
    copied.as_file().sync_all().unwrap();
    copied.publish(&archive).unwrap();
    let mut staged = store.stage().await.unwrap();
    staged.write(b"bytes".to_vec()).await.unwrap();
    staged.finish().await.unwrap();
    assert_eq!(staged.keep(hold.clone()).await, Err(StoreUnavailable));
    assert_eq!(fs::read(primary).unwrap(), original);
    let id = ArtifactId::from_generation(old.as_str());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await,
        Err(ArtifactReadError::Unavailable)
    );
}

#[tokio::test]
async fn retirement_success_survives_a_separate_blob_cleanup_failure_and_retry() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"result", b"result");
    let claim = claim(&store, &hold, b"result").await;
    store.confirm(&hold, &claim).await.unwrap();
    let blob = root
        .path()
        .join("attachments")
        .join(blob_path(hold.stored().digest()));
    // A real filesystem obstruction makes remove_file fail deterministically,
    // independent of the test runner's OS user or permission overrides.
    fs::remove_file(&blob).unwrap();
    fs::create_dir(&blob).unwrap();
    assert_eq!(
        store
            .discard(&hold, &claim, RevertCause::ConfirmationFailed)
            .await,
        Ok(Discard::CleanupIncomplete {
            was: RetiredFrom::Held
        })
    );
    let record = root.path().join("attachments").join(path_of(&hold));
    let first_evidence = fs::read(&record).unwrap();
    let id = ArtifactId::from_generation(claim.as_str());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
    assert_eq!(
        store
            .discard(&hold, &claim, RevertCause::AuditUnconfirmed)
            .await,
        Ok(Discard::NotMine)
    );
    assert_eq!(fs::read(record).unwrap(), first_evidence);
    assert!(blob.is_dir());
}

#[tokio::test]
async fn a_correct_generation_cannot_retire_another_hold_description() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let actual = hold_for("org", CONVERSATION, b"actual upload", b"same stored bytes");
    let other = hold_for("org", CONVERSATION, b"another upload", b"same stored bytes");
    let claim = claim(&store, &actual, b"same stored bytes").await;
    store.confirm(&actual, &claim).await.unwrap();
    assert_eq!(
        store
            .discard(&other, &claim, RevertCause::ConfirmationFailed)
            .await,
        Ok(Discard::NotMine)
    );
    assert!(holds(&store, &actual).await);
}

#[tokio::test]
async fn an_uncertain_retirement_does_not_publish_a_manifest_until_directory_sync_succeeds() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let claim = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &claim).await.unwrap();
    *store.files.publication_fault.lock().unwrap() = Some(PublicationFault::AfterPrimaryReplace);
    let report = store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    assert_eq!(report.failures, 1);
    assert!(report.released.is_empty());
    assert!(report.removed.is_empty());
    assert!(store.read(digest_of(b"bytes"), 16).await.unwrap().is_some());
    let id = ArtifactId::from_generation(claim.as_str());
    *store.files.publication_fault.lock().unwrap() = Some(PublicationFault::BeforeManifestSync);
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await,
        Err(ArtifactReadError::Unavailable)
    );
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
    drop(store);
    let store = open(root.path());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
    let retried = store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    assert!(retried.released.is_empty());
    assert_eq!(retried.removed, vec![hold]);
}

#[tokio::test]
async fn an_uncertain_archive_move_creates_no_fresh_pending_registration() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let old = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &old).await.unwrap();
    store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    *store.files.publication_fault.lock().unwrap() = Some(PublicationFault::AfterArchiveMove);
    let mut staged = store.stage().await.unwrap();
    staged.write(b"bytes".to_vec()).await.unwrap();
    staged.finish().await.unwrap();
    assert_eq!(staged.keep(hold.clone()).await, Err(StoreUnavailable));
    assert!(store.files.read_record(&path_of(&hold)).unwrap().is_none());
    let id = ArtifactId::from_generation(old.as_str());
    drop(store);
    let store = open(root.path());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
    let new = claim(&store, &hold, b"bytes").await;
    assert_ne!(old, new);
    store.confirm(&hold, &new).await.unwrap();
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
}

#[test]
fn retirement_codec_is_exhaustive_and_refuses_other_states_or_mismatched_initiators() {
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let generation = "a-saved-generation";
    for (was, stored_spelling) in [
        (RetiredFrom::Pending, "pending"),
        (RetiredFrom::Held, "kept"),
    ] {
        for cause in [
            RevertCause::AuditUnconfirmed,
            RevertCause::ConfirmationFailed,
            RevertCause::RemovedBeforeUsable,
            RevertCause::UploadUnresolved,
            RevertCause::ConversationDeleted,
            RevertCause::ConversationNotFound,
        ] {
            let state = RecordState::Retired {
                was,
                evidence: RetirementEvidence::RevertedUpload {
                    cause,
                    caller: hold.uploaded_by().clone(),
                },
            };
            let bytes = encode(&hold, state.clone(), generation);
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["was"], stored_spelling);
            let saved = decode(&bytes).unwrap();
            assert_eq!(saved.state, state);
            assert_eq!(saved.hold, hold);
            assert_eq!(saved.generation, generation);
            let mut wrong: Value = serde_json::from_slice(&bytes).unwrap();
            wrong["retirement"]["caller"]["actionId"] = serde_json::json!("another-action");
            assert!(decode(&serde_json::to_vec(&wrong).unwrap()).is_none());
            wrong = serde_json::from_slice(&bytes).unwrap();
            wrong["state"] = serde_json::json!("released");
            assert!(decode(&serde_json::to_vec(&wrong).unwrap()).is_none());
            wrong = serde_json::from_slice(&bytes).unwrap();
            wrong["was"] = serde_json::json!("absent");
            assert!(decode(&serde_json::to_vec(&wrong).unwrap()).is_none());
        }
    }
    let active = encode(&hold, RecordState::Kept, generation);
    let mut wrong: Value = serde_json::from_slice(&active).unwrap();
    wrong["retirement"] = serde_json::json!({"kind":"release"});
    assert!(decode(&serde_json::to_vec(&wrong).unwrap()).is_none());
}

#[test]
fn maximum_escaped_retirement_evidence_fits_the_record_read_bound() {
    let escaped = format!("{}s", "\u{0001}".repeat(Caller::MAX_BYTES - 1));
    let identifier = "\\".repeat(MAX_IDENTIFIER_BYTES);
    let caller = Caller::new(principal(&identifier), &escaped, &escaped).unwrap();
    let media = format!("a/{}", "a".repeat(MediaType::MAX_BYTES - 2));
    let attachment = attachment(b"bytes", &media);
    let hold = Hold::restore(
        organization(&identifier),
        conversation(CONVERSATION),
        attachment.clone(),
        attachment,
        caller.clone(),
        u64::MAX,
        u64::MAX,
    )
    .unwrap();
    let state = RecordState::Retired {
        was: RetiredFrom::Held,
        evidence: RetirementEvidence::Release(ReleaseEvidence {
            cause: ReleaseCause::ConversationDeleted,
            caller,
            requested_at_ms: u64::MAX,
        }),
    };
    let bytes = encode(&hold, state.clone(), &"\u{0001}".repeat(64));
    assert!(bytes.len() as u64 <= MAX_HOLD_BYTES, "{}", bytes.len());
    assert_eq!(decode(&bytes).unwrap().state, state);
}

#[tokio::test]
async fn a_same_process_retry_confirms_uncertain_archive_durability_before_fresh_pending() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let old = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &old).await.unwrap();
    store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    *store.files.publication_fault.lock().unwrap() = Some(PublicationFault::AfterArchiveMove);
    let mut staged = store.stage().await.unwrap();
    staged.write(b"bytes".to_vec()).await.unwrap();
    staged.finish().await.unwrap();
    assert_eq!(staged.keep(hold.clone()).await, Err(StoreUnavailable));
    assert!(store.files.read_record(&path_of(&hold)).unwrap().is_none());
    *store.files.publication_fault.lock().unwrap() =
        Some(PublicationFault::BeforePendingDirectorySync);
    let mut staged = store.stage().await.unwrap();
    staged.write(b"bytes".to_vec()).await.unwrap();
    staged.finish().await.unwrap();
    assert_eq!(staged.keep(hold.clone()).await, Err(StoreUnavailable));
    assert!(store.files.read_record(&path_of(&hold)).unwrap().is_none());
    let new = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &new).await.unwrap();
    let id = ArtifactId::from_generation(old.as_str());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await
            .unwrap(),
        Some(ArtifactState::Deleted)
    );
}

#[test]
fn typed_saved_records_refuse_duplicate_keys_and_explicit_null_retirement_fields() {
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let active = String::from_utf8(encode(&hold, RecordState::Kept, "saved-generation")).unwrap();
    for value in [
        "\"state\":\"pending\",\"state\":\"kept\"",
        "\"state\":\"kept\",\"state\":\"pending\"",
        "\"state\":\"kept\",\"state\":\"kept\"",
    ] {
        assert!(decode(active.replacen("\"state\":\"kept\"", value, 1).as_bytes()).is_none());
    }
    for value in [
        "\"actionId\":\"other\",\"actionId\":\"begin-1\"",
        "\"actionId\":\"begin-1\",\"actionId\":\"other\"",
        "\"actionId\":\"begin-1\",\"actionId\":\"begin-1\"",
    ] {
        assert!(decode(
            active
                .replacen("\"actionId\":\"begin-1\"", value, 1)
                .as_bytes()
        )
        .is_none());
    }
    let retired = String::from_utf8(encode(
        &hold,
        RecordState::Retired {
            was: RetiredFrom::Held,
            evidence: RetirementEvidence::Release(release_evidence()),
        },
        "saved-generation",
    ))
    .unwrap();
    assert!(decode(retired.as_bytes()).is_some());
    for value in [
        "\"cause\":\"conversation_closed\",\"cause\":\"conversation_deleted\"",
        "\"cause\":\"conversation_deleted\",\"cause\":\"conversation_closed\"",
        "\"cause\":\"conversation_closed\",\"cause\":\"conversation_closed\"",
    ] {
        assert!(decode(
            retired
                .replacen("\"cause\":\"conversation_closed\"", value, 1)
                .as_bytes()
        )
        .is_none());
    }
    let mut value: Value = serde_json::from_str(&active).unwrap();
    value["was"] = Value::Null;
    assert!(decode(&serde_json::to_vec(&value).unwrap()).is_none());
    value.as_object_mut().unwrap().remove("was");
    value["retirement"] = Value::Null;
    assert!(decode(&serde_json::to_vec(&value).unwrap()).is_none());
}

#[tokio::test]
async fn an_identical_retired_archive_is_idempotent_but_an_active_archive_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let old = claim(&store, &hold, b"bytes").await;
    store.confirm(&hold, &old).await.unwrap();
    let active = fs::read(root.path().join("attachments").join(path_of(&hold))).unwrap();
    store
        .release(
            hold.organization_id(),
            hold.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    let primary = root.path().join("attachments").join(path_of(&hold));
    let retired = fs::read(&primary).unwrap();
    let archive = root
        .path()
        .join("attachments")
        .join(archive_path(&hold, old.as_str()));
    let relative = archive_path(&hold, old.as_str());
    let mut copied =
        PrivateTempFile::new_beneath(&store.files.root, relative.parent().unwrap()).unwrap();
    copied.as_file_mut().write_all(&retired).unwrap();
    copied.as_file().sync_all().unwrap();
    copied.publish(&archive).unwrap();
    let id = ArtifactId::from_generation(old.as_str());
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await,
        Ok(Some(ArtifactState::Deleted))
    );
    let new = claim(&store, &hold, b"bytes").await;
    assert_ne!(new, old);
    assert_eq!(fs::read(&archive).unwrap(), retired);
    store.confirm(&hold, &new).await.unwrap();
    // A file at the archive name is not authority: its record must actually be
    // Retired and its embedded owner/generation must agree with that path.
    fs::write(&archive, active).unwrap();
    assert_eq!(
        store
            .manifest(hold.organization_id(), hold.conversation_id(), &id)
            .await,
        Err(ArtifactReadError::Unavailable)
    );
}

#[tokio::test]
async fn an_unrelated_corrupt_primary_prevents_live_manifest_without_releasing_its_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let first = hold_for("org", CONVERSATION, b"first", b"first");
    let second = hold_for("org", CONVERSATION, b"second", b"second");
    let first_claim = claim(&store, &first, b"first").await;
    let second_claim = claim(&store, &second, b"second").await;
    store.confirm(&first, &first_claim).await.unwrap();
    store.confirm(&second, &second_claim).await.unwrap();
    let id = ArtifactId::from_generation(first_claim.as_str());
    assert!(matches!(
        store
            .manifest(first.organization_id(), first.conversation_id(), &id)
            .await,
        Ok(Some(ArtifactState::Live(_)))
    ));
    fs::write(
        root.path().join("attachments").join(path_of(&second)),
        b"invalid saved record",
    )
    .unwrap();
    // This must fail even if the exact Live match was enumerated first.
    assert_eq!(
        store
            .manifest(first.organization_id(), first.conversation_id(), &id)
            .await,
        Err(ArtifactReadError::Unavailable)
    );
    let report = store
        .release(
            first.organization_id(),
            first.conversation_id(),
            &release_evidence(),
        )
        .await
        .unwrap();
    assert_eq!(report.failures, 1);
    assert_eq!(report.released.len(), 1);
    assert!(store
        .read(digest_of(b"second"), 16)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        store
            .manifest(first.organization_id(), first.conversation_id(), &id)
            .await,
        Err(ArtifactReadError::Unavailable)
    );
}
