//! Exact held-lifetime manifests and durable retirement/reupload boundaries.
use super::*;
use crate::attachments::application::{
    ArtifactReadError, ArtifactState, AttachmentArtifacts, AttachmentDependencies,
    AttachmentLimits, AttachmentService, ReleaseError, ReleaseRequest,
};
use crate::attachments::infrastructure::DurableAttachmentAudit;
use crate::attachments_test_support::Fixture;
use nessa_auth::domain::MAX_IDENTIFIER_BYTES;
use serde_json::Value;
use std::sync::Mutex;

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
    assert_eq!(
        report.retired.len(),
        1,
        "scan confirmed the saved retirement"
    );
    assert_eq!(report.removed.len(), 1);
    assert!(store.read(digest_of(b"bytes"), 16).await.unwrap().is_none());
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
    assert_eq!(retried.retired.len(), 1);
    assert_eq!(retried.retired[0].was(), RetiredFrom::Held);
    assert_eq!(
        retried.retired[0].evidence(),
        &RetirementEvidence::Release(release_evidence())
    );
    assert!(
        retried.removed.is_empty(),
        "first scan already removed the blob"
    );
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
    assert_eq!(report.retired.len(), 1);
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

// Actual local store and durable sink; only unrelated application ports are doubles.
async fn retry_retirement_case(kept: bool, reopen: bool, audit_refuses: bool, reversal: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut store = Arc::new(open(root.path()));
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let claim = claim(&store, &hold, b"bytes").await;
    if kept {
        store.confirm(&hold, &claim).await.unwrap();
    }
    let was = if kept {
        RetiredFrom::Held
    } else {
        RetiredFrom::Pending
    };
    let support = Fixture::new(AttachmentLimits::default());
    support.clock.set(3_000);
    let audit_path = root.path().join("audit");
    let audit =
        Arc::new(DurableAttachmentAudit::new(audit_path.clone(), support.clock.clone()).unwrap());
    let service_for = |store: Arc<LocalAttachmentStore>| {
        AttachmentService::new(
            AttachmentDependencies {
                store,
                audit: audit.clone(),
                ownership: support.ownership.clone(),
                secrets: support.secrets.clone(),
                normalizer: support.normalizer.clone(),
                clock: support.clock.clone(),
            },
            AttachmentLimits::default(),
        )
    };
    let request = |changed: bool| ReleaseRequest {
        organization_id: hold.organization_id().clone(),
        conversation_id: hold.conversation_id().clone(),
        cause: if changed {
            ReleaseCause::ConversationDeleted
        } else {
            ReleaseCause::ConversationClosed
        },
        principal_id: principal(if changed { "another-owner" } else { "owner" }),
        surface_id: if changed { "phone" } else { "panel" }.into(),
        correlation_id: if changed { "retry-2" } else { "release-1" }.into(),
    };
    let first = service_for(store.clone());
    let blob = root
        .path()
        .join("attachments")
        .join(blob_path(hold.stored().digest()));
    if reversal {
        fs::remove_file(&blob).unwrap();
        fs::create_dir(&blob).unwrap();
        assert_eq!(
            store
                .discard(&hold, &claim, RevertCause::ConfirmationFailed)
                .await,
            Ok(Discard::CleanupIncomplete { was })
        );
        fs::remove_dir(&blob).unwrap();
        fs::write(&blob, b"bytes").unwrap();
    } else {
        // Keep actual cleanup pending after the scan confirms the transiently
        // failed publication, so the changed-request retry removes bytes later.
        fs::remove_file(&blob).unwrap();
        fs::create_dir(&blob).unwrap();
        *store.files.publication_fault.lock().unwrap() =
            Some(PublicationFault::AfterPrimaryReplace);
        assert_eq!(
            first.release(request(false)).await,
            Err(ReleaseError::Incomplete {
                storage_failures: 2,
                audit_failures: 0
            })
        );
        fs::remove_dir(&blob).unwrap();
        fs::write(&blob, b"bytes").unwrap();
    }
    let saved_path = root.path().join("attachments").join(path_of(&hold));
    let original = fs::read(&saved_path).unwrap();
    drop(first);
    if reopen {
        drop(store);
        store = Arc::new(open(root.path()));
    }
    support.clock.set(9_000);
    if audit_refuses {
        fs::remove_dir_all(&audit_path).unwrap();
    }
    let service = service_for(store.clone());
    let result = service.release(request(true)).await;
    if audit_refuses {
        assert_eq!(
            result,
            Err(ReleaseError::Incomplete {
                storage_failures: 0,
                audit_failures: 2
            })
        );
    } else {
        result.unwrap();
    }
    assert!(
        !blob.exists(),
        "cleanup independent of audit acknowledgement"
    );
    assert_eq!(
        fs::read(&saved_path).unwrap(),
        original,
        "retry preserves exact original metadata"
    );
    if !audit_refuses {
        let records: Vec<Value> = fs::read_dir(&audit_path)
            .unwrap()
            .map(|entry| serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap())
            .collect();
        assert_eq!(records.len(), if reversal { 2 } else { 3 });
        for record in records {
            let fact = if record["kind"] == "attachment_bytes_removed" {
                assert_eq!(record["cause"], "unheld_cleanup");
                assert_eq!(record["initiator"]["kind"], "automatic");
                assert_eq!(
                    record["target"]["storedDigest"],
                    hold.stored().digest().to_string()
                );
                assert_eq!(
                    record["transition"],
                    serde_json::json!({"before":"stored","after":"absent"})
                );
                assert_eq!(record["retirements"].as_array().unwrap().len(), 1);
                &record["retirements"][0]
            } else {
                &record
            };
            assert_eq!(fact["target"]["conversationId"], CONVERSATION);
            assert_eq!(
                fact["transition"]["before"],
                if kept { "held" } else { "pending" }
            );
            assert_eq!(fact["transition"]["after"], "absent");
            if reversal {
                assert_eq!(fact["kind"], "attachment_hold_reverted");
                assert_eq!(fact["cause"], "confirmation_failed");
                assert_eq!(fact["initiator"]["kind"], "automatic");
                assert_eq!(fact["correlationId"], hold.uploaded_by().action_id());
                assert_eq!(fact["requestedAtMs"], hold.ticket_issued_at_ms());
            } else {
                assert_eq!(fact["kind"], "attachment_hold_released");
                assert_eq!(fact["cause"], "conversation_closed");
                assert_eq!(fact["initiator"]["principalId"], "owner");
                assert_eq!(fact["initiator"]["surfaceId"], "panel");
                assert_eq!(fact["correlationId"], "release-1");
                assert_eq!(fact["requestedAtMs"], 3_000);
            }
        }
    }
    drop(service);
    drop(store);
    let reopened = open(root.path());
    let saved = reopened
        .files
        .read_record(&path_of(&hold))
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.state,
        RecordState::Retired {
            was,
            evidence: if reversal {
                RetirementEvidence::RevertedUpload {
                    cause: RevertCause::ConfirmationFailed,
                    caller: hold.uploaded_by().clone(),
                }
            } else {
                RetirementEvidence::Release(release_evidence())
            }
        }
    );
}

#[tokio::test]
async fn changed_release_retry_keeps_original_retirement_in_actual_durable_audit() {
    for kept in [false, true] {
        for reopen in [false, true] {
            for audit_refuses in [false, true] {
                retry_retirement_case(kept, reopen, audit_refuses, false).await;
            }
        }
    }
}

#[tokio::test]
async fn reversal_cleanup_retry_stays_automatic_in_actual_durable_audit() {
    for kept in [false, true] {
        for reopen in [false, true] {
            for audit_refuses in [false, true] {
                retry_retirement_case(kept, reopen, audit_refuses, true).await;
            }
        }
    }
}

#[tokio::test]
async fn mixed_media_digest_cleanup_keeps_all_original_retirements_in_actual_durable_audit() {
    for reverse_creation in [false, true] {
        for all_prior in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let store = Arc::new(open(root.path()));
            let first = hold_for("org", CONVERSATION, b"bytes", b"bytes");
            let second_ticket = UploadTicket::new(
                organization("org"),
                conversation(CONVERSATION),
                attachment(b"bytes", "image/png"),
                Caller::new(principal("owner"), "panel", "begin-2").unwrap(),
                TicketLifetime::starting(1_000).unwrap(),
            );
            let second =
                Hold::from_upload(&second_ticket, attachment(b"bytes", "image/png"), 2_000)
                    .unwrap();
            let (first_claim, second_claim) = if reverse_creation {
                let two = claim(&store, &second, b"bytes").await;
                let one = claim(&store, &first, b"bytes").await;
                (one, two)
            } else {
                (
                    claim(&store, &first, b"bytes").await,
                    claim(&store, &second, b"bytes").await,
                )
            };
            store.confirm(&second, &second_claim).await.unwrap();
            let blob = root
                .path()
                .join("attachments")
                .join(blob_path(first.stored().digest()));
            // A second active primary still protects the shared bytes.
            assert_eq!(
                store
                    .discard(&first, &first_claim, RevertCause::ConfirmationFailed)
                    .await,
                Ok(Discard::Discarded {
                    was: RetiredFrom::Pending
                })
            );
            let first_path = root.path().join("attachments").join(path_of(&first));
            let first_original = fs::read(&first_path).unwrap();
            let original_release = release_evidence();
            if all_prior {
                fs::remove_file(&blob).unwrap();
                fs::create_dir(&blob).unwrap();
                let prior = store
                    .release(
                        first.organization_id(),
                        first.conversation_id(),
                        &original_release,
                    )
                    .await
                    .unwrap();
                assert_eq!(prior.retired.len(), 2);
                assert!(prior.removed.is_empty());
                assert_eq!(prior.failures, 1);
                fs::remove_dir(&blob).unwrap();
                fs::write(&blob, b"bytes").unwrap();
            }
            let support = Fixture::new(AttachmentLimits::default());
            support.clock.set(9_000);
            let audit_path = root.path().join("audit");
            let audit = Arc::new(
                DurableAttachmentAudit::new(audit_path.clone(), support.clock.clone()).unwrap(),
            );
            let service = AttachmentService::new(
                AttachmentDependencies {
                    store: store.clone(),
                    audit,
                    ownership: support.ownership.clone(),
                    secrets: support.secrets.clone(),
                    normalizer: support.normalizer.clone(),
                    clock: support.clock.clone(),
                },
                AttachmentLimits::default(),
            );
            service
                .release(ReleaseRequest {
                    organization_id: first.organization_id().clone(),
                    conversation_id: first.conversation_id().clone(),
                    cause: ReleaseCause::ConversationDeleted,
                    principal_id: principal("another-owner"),
                    surface_id: "phone".into(),
                    correlation_id: "retry-2".into(),
                })
                .await
                .unwrap();
            assert!(!blob.exists());
            assert_eq!(fs::read(&first_path).unwrap(), first_original);
            let records: Vec<Value> = fs::read_dir(&audit_path)
                .unwrap()
                .map(|entry| {
                    serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap()
                })
                .collect();
            assert_eq!(records.len(), 3);
            let removals: Vec<_> = records
                .iter()
                .filter(|record| record["kind"] == "attachment_bytes_removed")
                .collect();
            assert_eq!(removals.len(), 1, "one actual digest removal");
            let removal = removals[0];
            assert_eq!(removal["cause"], "unheld_cleanup");
            assert_eq!(removal["initiator"]["kind"], "automatic");
            assert_eq!(
                removal["target"]["storedDigest"],
                first.stored().digest().to_string()
            );
            let related = removal["retirements"].as_array().unwrap();
            assert_eq!(related.len(), 2);
            let reversal = related
                .iter()
                .find(|record| record["target"]["stored"]["mediaType"] == PDF)
                .unwrap();
            assert_eq!(reversal["kind"], "attachment_hold_reverted");
            assert_eq!(reversal["cause"], "confirmation_failed");
            assert_eq!(reversal["initiator"]["kind"], "automatic");
            assert_eq!(reversal["transition"]["before"], "pending");
            assert_eq!(reversal["correlationId"], "begin-1");
            assert_eq!(reversal["requestedAtMs"], 1_000);
            let release = related
                .iter()
                .find(|record| record["target"]["stored"]["mediaType"] == "image/png")
                .unwrap();
            assert_eq!(release["kind"], "attachment_hold_released");
            assert_eq!(release["transition"]["before"], "held");
            assert_eq!(
                release["cause"],
                if all_prior {
                    "conversation_closed"
                } else {
                    "conversation_deleted"
                }
            );
            assert_eq!(
                release["correlationId"],
                if all_prior { "release-1" } else { "retry-2" }
            );
            assert_eq!(
                release["requestedAtMs"],
                if all_prior { 3_000 } else { 9_000 }
            );
            assert_eq!(
                release["initiator"]["principalId"],
                if all_prior { "owner" } else { "another-owner" }
            );
            for fact in related {
                assert!(records.iter().any(|record| record["kind"] == fact["kind"]
                    && record["target"] == fact["target"]
                    && record["transition"] == fact["transition"]
                    && record["cause"] == fact["cause"]
                    && record["correlationId"] == fact["correlationId"]));
            }
            let repeated = store
                .release(
                    first.organization_id(),
                    first.conversation_id(),
                    &original_release,
                )
                .await
                .unwrap();
            assert_eq!(repeated.retired.len(), 2);
            assert!(repeated.removed.is_empty());
            assert_eq!(repeated.failures, 0);
        }
    }
}

#[tokio::test]
async fn transiently_confirmed_shared_digest_retirements_all_reach_actual_cleanup_audit() {
    for reversal in [false, true] {
        for reverse in [false, true] {
            for reopen in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let mut store = Arc::new(open(root.path()));
                let first = hold_for("org", CONVERSATION, b"bytes", b"bytes");
                let ticket = UploadTicket::new(
                    organization("org"),
                    conversation(CONVERSATION),
                    attachment(b"bytes", "image/png"),
                    Caller::new(principal("owner"), "panel", "begin-2").unwrap(),
                    TicketLifetime::starting(1_000).unwrap(),
                );
                let second =
                    Hold::from_upload(&ticket, attachment(b"bytes", "image/png"), 2_000).unwrap();
                let (first_claim, second_claim) = if reverse {
                    let second_claim = claim(&store, &second, b"bytes").await;
                    (claim(&store, &first, b"bytes").await, second_claim)
                } else {
                    (
                        claim(&store, &first, b"bytes").await,
                        claim(&store, &second, b"bytes").await,
                    )
                };
                store.confirm(&second, &second_claim).await.unwrap();
                if reversal {
                    assert_eq!(
                        store
                            .discard(&first, &first_claim, RevertCause::ConfirmationFailed)
                            .await,
                        Ok(Discard::Discarded {
                            was: RetiredFrom::Pending
                        })
                    );
                }
                let support = Fixture::new(AttachmentLimits::default());
                support.clock.set(3_000);
                let audit_path = root.path().join("audit");
                let audit = Arc::new(
                    DurableAttachmentAudit::new(audit_path.clone(), support.clock.clone()).unwrap(),
                );
                let service_for = |store: Arc<LocalAttachmentStore>| {
                    AttachmentService::new(
                        AttachmentDependencies {
                            store,
                            audit: audit.clone(),
                            ownership: support.ownership.clone(),
                            secrets: support.secrets.clone(),
                            normalizer: support.normalizer.clone(),
                            clock: support.clock.clone(),
                        },
                        AttachmentLimits::default(),
                    )
                };
                let request = |retry| ReleaseRequest {
                    organization_id: first.organization_id().clone(),
                    conversation_id: first.conversation_id().clone(),
                    cause: if retry {
                        ReleaseCause::ConversationDeleted
                    } else {
                        ReleaseCause::ConversationClosed
                    },
                    principal_id: principal(if retry { "another-owner" } else { "owner" }),
                    surface_id: if retry { "phone" } else { "panel" }.into(),
                    correlation_id: if retry { "retry-2" } else { "release-1" }.into(),
                };
                *store.files.publication_fault.lock().unwrap() =
                    Some(PublicationFault::AfterPrimaryReplace);
                let service = service_for(store.clone());
                assert_eq!(
                    service.release(request(false)).await,
                    Err(ReleaseError::Incomplete {
                        storage_failures: 1,
                        audit_failures: 0
                    })
                );
                let blob = root
                    .path()
                    .join("attachments")
                    .join(blob_path(first.stored().digest()));
                assert!(!blob.exists());
                let records: Vec<Value> = fs::read_dir(&audit_path)
                    .unwrap()
                    .map(|entry| {
                        serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap()
                    })
                    .collect();
                assert_eq!(
                    records.len(),
                    3,
                    "two original retirements and one physical removal"
                );
                let removal = records
                    .iter()
                    .find(|record| record["kind"] == "attachment_bytes_removed")
                    .unwrap();
                let contributors = removal["retirements"].as_array().unwrap();
                assert_eq!(
                    contributors.len(),
                    2,
                    "scan-confirmed failed write is load-bearing"
                );
                for (media, before, cause, correlation) in [
                    (
                        PDF,
                        "pending",
                        if reversal {
                            "confirmation_failed"
                        } else {
                            "conversation_closed"
                        },
                        if reversal { "begin-1" } else { "release-1" },
                    ),
                    ("image/png", "held", "conversation_closed", "release-1"),
                ] {
                    let fact = contributors
                        .iter()
                        .find(|fact| fact["target"]["stored"]["mediaType"] == media)
                        .unwrap();
                    assert_eq!(fact["transition"]["before"], before);
                    assert_eq!(fact["cause"], cause);
                    assert_eq!(fact["correlationId"], correlation);
                }
                let paths = [
                    root.path().join("attachments").join(path_of(&first)),
                    root.path().join("attachments").join(path_of(&second)),
                ];
                let saved = paths
                    .iter()
                    .map(|path| fs::read(path).unwrap())
                    .collect::<Vec<_>>();
                drop(service);
                if reopen {
                    drop(store);
                    store = Arc::new(open(root.path()));
                }
                support.clock.set(9_000);
                service_for(store.clone())
                    .release(request(true))
                    .await
                    .unwrap();
                for (path, original) in paths.iter().zip(saved) {
                    assert_eq!(fs::read(path).unwrap(), original);
                }
                let records: Vec<Value> = fs::read_dir(&audit_path)
                    .unwrap()
                    .map(|entry| {
                        serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap()
                    })
                    .collect();
                assert_eq!(
                    records
                        .iter()
                        .filter(|record| record["kind"] == "attachment_bytes_removed")
                        .count(),
                    1,
                    "retry cannot fabricate another physical removal"
                );
            }
        }
    }
}

#[tokio::test]
async fn confirmed_retirement_survives_conservative_real_primary_reread_failure() {
    for kept in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(open(root.path()));
        let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
        let claim = claim(&store, &hold, b"bytes").await;
        if kept {
            store.confirm(&hold, &claim).await.unwrap();
        }
        let path = root.path().join("attachments").join(path_of(&hold));
        let saved = Arc::new(Mutex::new(None));
        let retained = saved.clone();
        let reread_path = path.clone();
        *store.files.before_retention_scan.lock().unwrap() = Some(Box::new(move || {
            *retained.lock().unwrap() = Some(fs::read(&reread_path).unwrap());
            // A real private primary becomes undecodable only after acknowledgement.
            // The scan must retain its filename digest without losing the known fact.
            fs::write(&reread_path, b"unreadable record").unwrap();
        }));
        let support = Fixture::new(AttachmentLimits::default());
        support.clock.set(3_000);
        let audit_path = root.path().join("audit");
        let service = AttachmentService::new(
            AttachmentDependencies {
                store: store.clone(),
                audit: Arc::new(
                    DurableAttachmentAudit::new(audit_path.clone(), support.clock.clone()).unwrap(),
                ),
                ownership: support.ownership.clone(),
                secrets: support.secrets.clone(),
                normalizer: support.normalizer.clone(),
                clock: support.clock.clone(),
            },
            AttachmentLimits::default(),
        );
        let request = |retry| ReleaseRequest {
            organization_id: hold.organization_id().clone(),
            conversation_id: hold.conversation_id().clone(),
            cause: if retry {
                ReleaseCause::ConversationDeleted
            } else {
                ReleaseCause::ConversationClosed
            },
            principal_id: principal(if retry { "another-owner" } else { "owner" }),
            surface_id: "panel".into(),
            correlation_id: if retry { "retry-2" } else { "release-1" }.into(),
        };
        assert!(matches!(
            service.release(request(false)).await,
            Err(ReleaseError::Incomplete {
                storage_failures: 1,
                audit_failures: 0
            })
        ));
        let records: Vec<Value> = fs::read_dir(&audit_path)
            .unwrap()
            .map(|entry| serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap())
            .collect();
        assert_eq!(
            records.len(),
            1,
            "known confirmed retirement must survive reread failure"
        );
        assert_eq!(records[0]["kind"], "attachment_hold_released");
        assert_eq!(
            records[0]["transition"]["before"],
            if kept { "held" } else { "pending" }
        );
        assert_eq!(records[0]["cause"], "conversation_closed");
        assert_eq!(records[0]["correlationId"], "release-1");
        let blob = root
            .path()
            .join("attachments")
            .join(blob_path(hold.stored().digest()));
        assert!(
            blob.exists(),
            "unreadable primary conservatively retains bytes"
        );
        fs::write(&path, saved.lock().unwrap().take().unwrap()).unwrap();
        support.clock.set(9_000);
        service.release(request(true)).await.unwrap();
        assert!(!blob.exists());
        let records: Vec<Value> = fs::read_dir(&audit_path)
            .unwrap()
            .map(|entry| serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap())
            .collect();
        let removal = records
            .iter()
            .find(|record| record["kind"] == "attachment_bytes_removed")
            .unwrap();
        assert_eq!(removal["retirements"][0]["cause"], "conversation_closed");
        assert_eq!(removal["retirements"][0]["correlationId"], "release-1");
    }
}

#[tokio::test]
async fn cleanup_distinguishes_active_unrelated_and_candidate_uncertain_retention() {
    for kept in [false, true] {
        for discard in [false, true] {
            // 0=unheld, 1=valid foreign holder, 2=unrelated corruption,
            // 3=unknown candidate, 4=valid holder plus unknown candidate.
            for case in 0..5 {
                let root = tempfile::tempdir().unwrap();
                let store = open(root.path());
                let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
                let owned = claim(&store, &hold, b"bytes").await;
                if kept {
                    store.confirm(&hold, &owned).await.unwrap();
                }
                let active = if case == 1 || case == 4 {
                    let active = hold_for("org", OTHER_CONVERSATION, b"bytes", b"bytes");
                    let active_claim = claim(&store, &active, b"bytes").await;
                    store.confirm(&active, &active_claim).await.unwrap();
                    Some(active)
                } else {
                    None
                };
                let uncertain = if case >= 2 {
                    let bytes: &[u8] = if case == 2 { b"unrelated" } else { b"bytes" };
                    let unknown = hold_for("foreign", OTHER_CONVERSATION, bytes, bytes);
                    let _original_claim = claim(&store, &unknown, bytes).await;
                    let path = root.path().join("attachments").join(path_of(&unknown));
                    let original = fs::read(&path).unwrap();
                    let physical_path = path.clone();
                    *store.files.before_retention_scan.lock().unwrap() =
                        Some(Box::new(move || {
                            fs::write(physical_path, b"unreadable primary").unwrap();
                        }));
                    Some((unknown, path, original))
                } else {
                    None
                };
                let candidate_unknown = case == 3 || case == 4;
                if discard {
                    assert_eq!(
                        store
                            .discard(&hold, &owned, RevertCause::ConfirmationFailed)
                            .await,
                        Ok(if candidate_unknown {
                            Discard::CleanupIncomplete {
                                was: if kept {
                                    RetiredFrom::Held
                                } else {
                                    RetiredFrom::Pending
                                },
                            }
                        } else {
                            Discard::Discarded {
                                was: if kept {
                                    RetiredFrom::Held
                                } else {
                                    RetiredFrom::Pending
                                },
                            }
                        }),
                        "case {case}"
                    );
                } else {
                    let report = store
                        .release(
                            hold.organization_id(),
                            hold.conversation_id(),
                            &release_evidence(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(
                        report.failures,
                        usize::from(candidate_unknown),
                        "case {case}"
                    );
                    assert_eq!(report.retired.len(), 1);
                    assert_eq!(report.retired[0].hold(), &hold);
                    assert_eq!(report.removed.len(), usize::from(case == 0 || case == 2));
                }
                let blob = root
                    .path()
                    .join("attachments")
                    .join(blob_path(hold.stored().digest()));
                assert_eq!(blob.exists(), case == 1 || candidate_unknown, "case {case}");
                if let Some(active) = &active {
                    assert!(holds(&store, active).await);
                }
                if let Some((unknown, path, original)) = uncertain {
                    // Actual restore establishes retention again; it does not
                    // invent retirement or remove another conversation's hold.
                    fs::write(path, original).unwrap();
                    let retry = store
                        .release(
                            hold.organization_id(),
                            hold.conversation_id(),
                            &release_evidence(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(retry.failures, 0);
                    assert_eq!(retry.retired.len(), 1);
                    assert_eq!(retry.retired[0].hold(), &hold);
                    if discard {
                        assert!(
                            matches!(retry.retired[0].evidence(), RetirementEvidence::RevertedUpload {
                            cause: RevertCause::ConfirmationFailed, caller
                        } if caller == hold.uploaded_by())
                        );
                    } else {
                        assert_eq!(
                            retry.retired[0].evidence(),
                            &RetirementEvidence::Release(release_evidence())
                        );
                    }
                    assert_eq!(retry.removed.len(), 0, "no repeat or foreign removal");
                    assert!(root
                        .path()
                        .join("attachments")
                        .join(blob_path(unknown.stored().digest()))
                        .exists());
                }
            }
        }
    }
}

/// A published hold is saved with the lease that published it and comes
/// back with it; an upload is saved and restored with none. A lease record
/// whose file changed between arriving and being kept describes no publish.
#[test]
fn a_saved_hold_keeps_the_lease_that_published_it_and_only_that_one() {
    let file = attachment(b"bytes", PDF);
    let published = Hold::published(
        organization("org"),
        conversation(CONVERSATION),
        file,
        Caller::new(principal("owner"), "phone", "request-1").unwrap(),
        "lease-1",
        1_000,
        2_000,
    )
    .unwrap();
    let bytes = encode(&published, RecordState::Kept, "saved-generation");
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["lease"], "lease-1");
    let saved = decode(&bytes).unwrap();
    assert_eq!(saved.hold, published);
    assert_eq!(saved.hold.lease(), Some("lease-1"));

    let uploaded = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let bytes = encode(&uploaded, RecordState::Kept, "saved-generation");
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("lease").is_none(), "{value}");
    let saved = decode(&bytes).unwrap();
    assert_eq!(saved.hold, uploaded);
    assert_eq!(saved.hold.lease(), None);

    let mut changed: Value =
        serde_json::from_slice(&encode(&published, RecordState::Kept, "saved-generation")).unwrap();
    changed["uploaded"]["digest"] = serde_json::json!(digest_of(b"other").to_string());
    assert!(decode(&serde_json::to_vec(&changed).unwrap()).is_none());
    let normalized = hold_for("org", CONVERSATION, b"photograph", b"normalized");
    let mut leased: Value =
        serde_json::from_slice(&encode(&normalized, RecordState::Kept, "saved-generation"))
            .unwrap();
    leased["lease"] = serde_json::json!("lease-1");
    assert!(decode(&serde_json::to_vec(&leased).unwrap()).is_none());
}

/// A saved published hold comes back only with a lease id a conversation's
/// record could name: the one rule `LeaseId` holds.
#[test]
fn a_saved_hold_whose_lease_no_record_could_name_is_not_restored() {
    let published = Hold::published(
        organization("org"),
        conversation(CONVERSATION),
        attachment(b"bytes", PDF),
        Caller::new(principal("owner"), "phone", "request-1").unwrap(),
        "lease-1",
        1_000,
        2_000,
    )
    .unwrap();
    for lease in ["lease 1", "lease/1", "läse", &"a".repeat(129)] {
        let mut saved: Value =
            serde_json::from_slice(&encode(&published, RecordState::Kept, "saved-generation"))
                .unwrap();
        saved["lease"] = serde_json::json!(lease);
        assert!(
            decode(&serde_json::to_vec(&saved).unwrap()).is_none(),
            "{lease}"
        );
    }
}
