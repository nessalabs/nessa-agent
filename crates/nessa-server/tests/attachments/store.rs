//! The real store on a real filesystem: privacy, hostile names, restart, and
//! uploads racing a release of the same bytes.
use super::*;
use crate::attachments::application::AttachmentStore;
use crate::attachments::domain::{Caller, MediaType, TicketLifetime, UploadTicket};
use crate::attachments_test_support::{
    attachment, conversation, digest_of, organization, principal, CONVERSATION, OTHER_CONVERSATION,
};
use std::sync::Arc;
use tokio::sync::Barrier;

const PDF: &str = "application/pdf";

fn open(root: &Path) -> LocalAttachmentStore {
    LocalAttachmentStore::open(root.join("attachments")).unwrap()
}
fn hold_for(organization_id: &str, conversation_id: &str, uploaded: &[u8], stored: &[u8]) -> Hold {
    let media_type = if uploaded == stored { PDF } else { "image/png" };
    let ticket = UploadTicket::new(
        organization(organization_id),
        conversation(conversation_id),
        attachment(uploaded, media_type),
        Caller::new(principal("owner"), "panel", "begin-1").unwrap(),
        TicketLifetime::starting(1_000).unwrap(),
    );
    Hold::from_upload(&ticket, attachment(stored, media_type), 2_000).unwrap()
}
/// Stage `stored` in two writes and keep it under `hold`: written, not yet usable.
async fn keep(store: &LocalAttachmentStore, hold: &Hold, stored: &[u8]) -> Kept {
    let mut staged = store.stage().await.unwrap();
    let (first, second) = stored.split_at(stored.len() / 2);
    staged.write(first.to_vec()).await.unwrap();
    staged.write(second.to_vec()).await.unwrap();
    let received = staged.finish().await.unwrap();
    assert_eq!(received.digest, digest_of(stored));
    assert_eq!(received.size, stored.len() as u64);
    assert_eq!(staged.read().await.unwrap(), stored);
    staged.keep(hold.clone()).await.unwrap()
}
async fn claim(store: &LocalAttachmentStore, hold: &Hold, stored: &[u8]) -> HoldClaim {
    match keep(store, hold, stored).await {
        Kept::Pending(claim) => claim,
        Kept::Existing(existing) => panic!("already kept: {existing:?}"),
    }
}
/// Keep and confirm, as an upload whose evidence was committed does.
async fn keep_usable(store: &LocalAttachmentStore, hold: &Hold, stored: &[u8]) {
    let claim = claim(store, hold, stored).await;
    assert_eq!(
        store.confirm(hold, &claim).await,
        Ok(Confirmation::Confirmed)
    );
}
async fn upload(store: &LocalAttachmentStore, conversation_id: &str, bytes: &[u8]) -> Hold {
    let hold = hold_for("org", conversation_id, bytes, bytes);
    keep_usable(store, &hold, bytes).await;
    hold
}
async fn holds(store: &LocalAttachmentStore, hold: &Hold) -> bool {
    store
        .holds(
            hold.organization_id(),
            hold.conversation_id(),
            hold.stored(),
        )
        .await
        .unwrap()
}
fn files_beneath(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_beneath(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[tokio::test]
async fn a_kept_upload_is_held_found_and_read_and_survives_a_restart() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"what was sent", b"what is kept");
    keep_usable(&store, &hold, b"what is kept").await;
    // A usable hold is never replaced: keeping the same file again, even as
    // the result of some other upload, finds the hold as it stands.
    let another = hold_for("org", CONVERSATION, b"another original", b"what is kept");
    assert_eq!(
        keep(&store, &another, b"what is kept").await,
        Kept::Existing(hold.clone())
    );
    drop(store);

    let store = open(root.path());
    assert!(holds(&store, &hold).await);
    assert_eq!(
        store
            .find_upload(
                hold.organization_id(),
                hold.conversation_id(),
                hold.uploaded()
            )
            .await
            .unwrap(),
        Some(hold.clone())
    );
    // The stored file was never uploaded, and the uploaded one is not kept.
    assert!(store
        .find_upload(
            hold.organization_id(),
            hold.conversation_id(),
            hold.stored()
        )
        .await
        .unwrap()
        .is_none());
    assert!(!store
        .holds(
            hold.organization_id(),
            hold.conversation_id(),
            hold.uploaded()
        )
        .await
        .unwrap());
    assert_eq!(
        store
            .read(hold.stored().digest(), 1024)
            .await
            .unwrap()
            .unwrap(),
        b"what is kept"
    );
    assert_eq!(
        store
            .read(hold.stored().digest(), 4)
            .await
            .unwrap()
            .unwrap(),
        b"what"
    );
    assert!(store
        .read(digest_of(b"what was sent"), 1024)
        .await
        .unwrap()
        .is_none());
    assert!(fs::read_dir(root.path().join("attachments/incoming"))
        .unwrap()
        .next()
        .is_none());
}

#[tokio::test]
async fn every_fact_of_a_hold_must_agree_before_it_answers() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = upload(&store, CONVERSATION, b"bytes").await;
    let asks = |organization_id: &str, conversation_id: &str, stored: Attachment| {
        let (organization_id, conversation_id) =
            (organization(organization_id), conversation(conversation_id));
        let store = &store;
        async move {
            store
                .holds(&organization_id, &conversation_id, &stored)
                .await
                .unwrap()
        }
    };
    assert!(asks("org", CONVERSATION, attachment(b"bytes", PDF)).await);
    assert!(!asks("org", CONVERSATION, attachment(b"bytes", "text/plain")).await);
    assert!(!asks("org", CONVERSATION, attachment(b"other", PDF)).await);
    assert!(!asks("org", OTHER_CONVERSATION, attachment(b"bytes", PDF)).await);
    // Organizations that differ only by case, or that look like a path, are
    // different owners with different directories.
    for other in ["Org", "ORG", "../org", "org/../org", "."] {
        assert!(!asks(other, CONVERSATION, attachment(b"bytes", PDF)).await);
    }
    // A digest with another size describes bytes that cannot exist.
    let impossible =
        Attachment::new(digest_of(b"bytes"), MediaType::parse(PDF).unwrap(), 6).unwrap();
    assert!(!asks("org", CONVERSATION, impossible).await);
    assert!(holds(&store, &hold).await);
}

#[tokio::test]
async fn a_hold_without_its_bytes_is_not_a_hold() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = upload(&store, CONVERSATION, b"bytes").await;
    fs::remove_file(
        root.path()
            .join("attachments/blobs")
            .join(digest_of(b"bytes").to_hex()),
    )
    .unwrap();
    assert!(!holds(&store, &hold).await);
    // So beginning again asks for the bytes instead of claiming to have them.
    assert!(store
        .find_upload(
            hold.organization_id(),
            hold.conversation_id(),
            hold.uploaded()
        )
        .await
        .unwrap()
        .is_none());
    assert!(store.read(digest_of(b"bytes"), 16).await.unwrap().is_none());
}

#[tokio::test]
async fn names_from_outside_never_leave_the_root() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    for organization_id in ["../../escape", "/etc", "a/b", "..", "org\\evil", "Ω"] {
        let hold = hold_for(organization_id, CONVERSATION, b"bytes", b"bytes");
        keep_usable(&store, &hold, b"bytes").await;
        assert!(holds(&store, &hold).await);
    }
    let outside: Vec<_> = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(outside, ["attachments"]);
    // Six owners, six records, one copy of the bytes; every name is derived.
    let holds_root = root.path().join("attachments/holds");
    let records = files_beneath(&holds_root);
    assert_eq!(records.len(), 6);
    for record in records {
        let relative = record.strip_prefix(&holds_root).unwrap();
        let names: Vec<_> = relative
            .iter()
            .map(|name| name.to_str().unwrap().to_owned())
            .collect();
        assert_eq!(names.len(), 3);
        assert!(names[0].len() == 64 && names[0].bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(names[1], CONVERSATION);
        // The stored digest, a tag for the media type, and nothing from outside.
        assert_eq!(names[2].len(), 64 + 1 + 16 + 5);
        assert!(names[2].starts_with(&format!("{}-", digest_of(b"bytes").to_hex())));
        assert!(names[2].ends_with(".json"));
    }
    assert_eq!(
        files_beneath(&root.path().join("attachments/blobs")).len(),
        1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn everything_written_is_private_and_what_is_not_private_is_refused_not_repaired() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = upload(&store, CONVERSATION, b"bytes").await;
    let attachments = root.path().join("attachments");
    for file in files_beneath(&attachments) {
        assert_eq!(mode(&file), 0o600, "{}", file.display());
        let mut directory = file.parent().unwrap();
        while directory.starts_with(&attachments) {
            assert_eq!(mode(directory), 0o700, "{}", directory.display());
            directory = directory.parent().unwrap();
        }
    }

    // Bytes someone else can read are not served, and are left as found.
    let blob = attachments.join("blobs").join(digest_of(b"bytes").to_hex());
    fs::set_permissions(&blob, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        store.read(digest_of(b"bytes"), 16).await,
        Err(StoreUnavailable)
    );
    assert_eq!(
        store
            .holds(
                hold.organization_id(),
                hold.conversation_id(),
                hold.stored()
            )
            .await,
        Err(StoreUnavailable)
    );
    assert_eq!(mode(&blob), 0o644);
    fs::set_permissions(&blob, fs::Permissions::from_mode(0o600)).unwrap();

    // A second name for the same bytes is refused the same way.
    fs::hard_link(&blob, attachments.join("blobs/alias")).unwrap();
    assert_eq!(
        store.read(digest_of(b"bytes"), 16).await,
        Err(StoreUnavailable)
    );
    fs::remove_file(attachments.join("blobs/alias")).unwrap();
    assert!(holds(&store, &hold).await);
    drop(store);

    // Neither is a directory that is not private.
    for name in ["", "blobs", "holds", "incoming"] {
        let directory = attachments.join(name);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(LocalAttachmentStore::open(attachments.clone()).is_err());
        assert_eq!(mode(&directory), 0o755);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert!(holds(&open(root.path()), &hold).await);
}

#[cfg(unix)]
#[tokio::test]
async fn a_linked_directory_is_not_followed() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let elsewhere = root.path().join("elsewhere");
    nessa_local_storage::create_directory(&elsewhere).unwrap();
    let organization_directory = root
        .path()
        .join("attachments/holds")
        .join(organization_directory(hold.organization_id()));
    std::os::unix::fs::symlink(&elsewhere, &organization_directory).unwrap();

    let mut staged = store.stage().await.unwrap();
    staged.write(b"bytes".to_vec()).await.unwrap();
    staged.finish().await.unwrap();
    assert_eq!(staged.keep(hold.clone()).await, Err(StoreUnavailable));
    assert!(fs::read_dir(&elsewhere).unwrap().next().is_none());
    // The bytes published for a hold that never came to exist are not left behind.
    assert!(store.read(digest_of(b"bytes"), 16).await.unwrap().is_none());
}

#[tokio::test]
async fn staged_bytes_that_are_not_the_described_file_are_never_published() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"described", b"described");
    for written in [b"something else".as_slice(), b"describe"] {
        let mut staged = store.stage().await.unwrap();
        staged.write(written.to_vec()).await.unwrap();
        staged.finish().await.unwrap();
        assert_eq!(staged.keep(hold.clone()).await, Err(StoreUnavailable));
    }
    // Nor a transfer nobody finished.
    let mut staged = store.stage().await.unwrap();
    staged.write(b"described".to_vec()).await.unwrap();
    assert_eq!(staged.keep(hold.clone()).await, Err(StoreUnavailable));

    let attachments = root.path().join("attachments");
    assert!(files_beneath(&attachments.join("blobs")).is_empty());
    assert!(files_beneath(&attachments.join("holds")).is_empty());
    assert!(files_beneath(&attachments.join("incoming")).is_empty());
}

#[tokio::test]
async fn an_abandoned_transfer_removes_its_file_and_a_crashed_one_is_swept_at_open() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let incoming = root.path().join("attachments/incoming");
    let mut staged = store.stage().await.unwrap();
    staged.write(b"partial".to_vec()).await.unwrap();
    assert_eq!(files_beneath(&incoming).len(), 1);
    drop(staged);
    assert!(files_beneath(&incoming).is_empty());

    // What a crash leaves: a transfer's file, and a half-written hold record.
    let hold = upload(&store, CONVERSATION, b"bytes").await;
    drop(store);
    let conversation_directory = root.path().join("attachments").join(hold_directory(
        hold.organization_id(),
        hold.conversation_id(),
    ));
    for leftover in [
        incoming.join(".nessa-crashed.tmp"),
        conversation_directory.join(".nessa-crashed.tmp"),
    ] {
        fs::write(leftover, b"junk").unwrap();
    }
    let store = open(root.path());
    assert!(files_beneath(&incoming).is_empty());
    assert_eq!(files_beneath(&conversation_directory).len(), 1);
    assert!(holds(&store, &hold).await);
}

#[tokio::test]
async fn release_lets_go_of_one_conversation_and_bytes_go_with_their_last_hold() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let first = upload(&store, CONVERSATION, b"shared").await;
    let second = upload(&store, OTHER_CONVERSATION, b"shared").await;
    let alone = upload(&store, OTHER_CONVERSATION, b"alone").await;
    let foreign = hold_for("other-org", OTHER_CONVERSATION, b"alone2", b"alone2");
    keep_usable(&store, &foreign, b"alone2").await;
    assert_eq!(
        files_beneath(&root.path().join("attachments/blobs")).len(),
        3
    );

    let mut report = store
        .release(&organization("org"), &conversation(OTHER_CONVERSATION))
        .await
        .unwrap();
    report
        .released
        .sort_by_key(|released| released.hold.stored().size());
    assert_eq!(
        report,
        ReleaseReport {
            released: vec![
                ReleasedHold {
                    hold: alone.clone(),
                    was: HoldState::Held
                },
                ReleasedHold {
                    hold: second.clone(),
                    was: HoldState::Held
                },
            ],
            // `shared` is still held by the other conversation.
            removed: vec![alone.clone()],
            failures: 0,
        }
    );
    assert!(!holds(&store, &second).await);
    assert!(!holds(&store, &alone).await);
    assert!(holds(&store, &first).await);
    // Same conversation identifier, another organization: untouched.
    assert!(holds(&store, &foreign).await);
    assert!(store.read(digest_of(b"alone"), 16).await.unwrap().is_none());

    let report = store
        .release(&organization("org"), &conversation(CONVERSATION))
        .await
        .unwrap();
    assert_eq!(report.removed, std::slice::from_ref(&first));
    assert!(store
        .read(digest_of(b"shared"), 16)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .release(&organization("org"), &conversation(CONVERSATION))
            .await
            .unwrap(),
        ReleaseReport::default()
    );
    // A restart agrees with all of it.
    drop(store);
    let store = open(root.path());
    assert!(!holds(&store, &first).await);
    assert!(holds(&store, &foreign).await);
}

#[tokio::test]
async fn a_record_that_cannot_be_read_is_reported_kept_and_never_costs_the_others() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let good = upload(&store, CONVERSATION, b"good").await;
    let damaged = upload(&store, CONVERSATION, b"damaged").await;
    // Valid JSON, valid values, but it claims to be another conversation's hold.
    let claimed = hold_for("org", OTHER_CONVERSATION, b"damaged", b"damaged");
    let record = root.path().join("attachments").join(path_of(&damaged));
    let forged = encode(&claimed, RecordState::Kept, "forged");
    fs::write(&record, &forged).unwrap();
    assert_eq!(
        store
            .holds(
                damaged.organization_id(),
                damaged.conversation_id(),
                damaged.stored()
            )
            .await,
        Err(StoreUnavailable)
    );
    // An unrelated upload still begins.
    assert!(store
        .find_upload(
            good.organization_id(),
            good.conversation_id(),
            good.uploaded()
        )
        .await
        .unwrap()
        .is_some());

    let report = store
        .release(&organization("org"), &conversation(CONVERSATION))
        .await
        .unwrap();
    assert_eq!(report.failures, 1);
    assert_eq!(report.released.len(), 1);
    assert_eq!(report.released[0].hold, good);
    assert_eq!(report.removed, std::slice::from_ref(&good));
    // The unreadable record is exactly as it was, and its bytes are still protected.
    assert_eq!(fs::read(&record).unwrap(), forged);
    assert_eq!(
        store
            .read(digest_of(b"damaged"), 16)
            .await
            .unwrap()
            .unwrap(),
        b"damaged"
    );
    for garbage in [b"{}".as_slice(), b"not json", &[b'x'; 9000]] {
        fs::write(&record, garbage).unwrap();
        assert_eq!(
            store
                .release(&organization("org"), &conversation(CONVERSATION))
                .await
                .unwrap()
                .failures,
            1
        );
    }
}

#[tokio::test]
async fn a_kept_record_that_describes_another_file_is_a_failure_not_an_answer() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    keep_usable(&store, &hold, b"bytes").await;

    // A record's name carries a digest and sixteen digits of the hash of a
    // media type, which is not the whole file: the same name also fits a
    // record claiming another length for those bytes. What is kept decides,
    // not where it is filed, so this one is refused rather than handed back
    // as a hold on bytes that cannot exist.
    let impossible =
        Attachment::new(digest_of(b"bytes"), MediaType::parse(PDF).unwrap(), 6).unwrap();
    let disagreeing = Hold::from_upload(
        &UploadTicket::new(
            organization("org"),
            conversation(CONVERSATION),
            impossible.clone(),
            Caller::new(principal("owner"), "panel", "begin-1").unwrap(),
            TicketLifetime::starting(1_000).unwrap(),
        ),
        impossible,
        2_000,
    )
    .unwrap();
    let record = root.path().join("attachments").join(path_of(&hold));
    let forged = encode(&disagreeing, RecordState::Kept, "forged");
    fs::write(&record, &forged).unwrap();

    let mut staged = store.stage().await.unwrap();
    staged.write(b"bytes".to_vec()).await.unwrap();
    staged.finish().await.unwrap();
    assert_eq!(staged.keep(hold.clone()).await, Err(StoreUnavailable));
    // Left exactly as it was found, for whoever investigates.
    assert_eq!(fs::read(&record).unwrap(), forged);
}

#[tokio::test]
async fn a_pending_hold_is_invisible_protects_its_bytes_and_answers_only_to_its_own_claim() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let claimed = claim(&store, &hold, b"bytes").await;
    // Written, but nothing that asks what the conversation holds can see it.
    assert!(!holds(&store, &hold).await);
    assert!(store
        .find_upload(
            hold.organization_id(),
            hold.conversation_id(),
            hold.uploaded()
        )
        .await
        .unwrap()
        .is_none());
    // Its bytes are protected all the same: another conversation letting go
    // of the same bytes does not take them.
    let other = upload(&store, OTHER_CONVERSATION, b"bytes").await;
    let report = store
        .release(other.organization_id(), other.conversation_id())
        .await
        .unwrap();
    assert!(report.removed.is_empty());
    assert_eq!(
        store.read(digest_of(b"bytes"), 16).await.unwrap().unwrap(),
        b"bytes"
    );

    // A claim nobody was given changes nothing.
    let stranger = HoldClaim::new("not-the-generation");
    assert_eq!(store.discard(&hold, &stranger).await, Ok(Discard::NotMine));
    // Its own claim takes it back, bytes and all, once.
    assert_eq!(store.discard(&hold, &claimed).await, Ok(Discard::Discarded));
    assert_eq!(store.discard(&hold, &claimed).await, Ok(Discard::NotMine));
    assert_eq!(store.confirm(&hold, &claimed).await, Ok(Confirmation::Gone));
    assert!(store.read(digest_of(b"bytes"), 16).await.unwrap().is_none());
    assert!(files_beneath(&root.path().join("attachments/holds")).is_empty());
}

#[tokio::test]
async fn taking_a_hold_back_undoes_that_upload_and_never_a_later_one_of_the_same_file() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let first = hold_for("org", CONVERSATION, b"first original", b"same result");
    let second = hold_for("org", CONVERSATION, b"second original", b"same result");

    // The later upload is recorded and confirmed while the earlier still waits.
    let earlier = claim(&store, &first, b"same result").await;
    let later = claim(&store, &second, b"same result").await;
    assert_eq!(
        store.confirm(&second, &later).await,
        Ok(Confirmation::Confirmed)
    );
    // The earlier one's evidence failed. Taking it back touches nothing.
    assert_eq!(store.discard(&first, &earlier).await, Ok(Discard::NotMine));
    assert!(holds(&store, &second).await);
    assert_eq!(
        store
            .find_upload(
                second.organization_id(),
                second.conversation_id(),
                second.uploaded()
            )
            .await
            .unwrap(),
        Some(second.clone())
    );
    // Had its evidence been committed instead, it would find the file kept.
    assert_eq!(
        store.confirm(&first, &earlier).await,
        Ok(Confirmation::AlreadyKept)
    );
    store
        .release(&organization("org"), &conversation(CONVERSATION))
        .await
        .unwrap();

    // The other order: the earlier upload's evidence lands first, so it takes
    // the hold over; the later one's failure then has nothing to undo.
    let earlier = claim(&store, &first, b"same result").await;
    let later = claim(&store, &second, b"same result").await;
    assert_eq!(
        store.confirm(&first, &earlier).await,
        Ok(Confirmation::Confirmed)
    );
    assert_eq!(store.discard(&second, &later).await, Ok(Discard::NotMine));
    assert_eq!(
        store
            .find_upload(
                first.organization_id(),
                first.conversation_id(),
                first.uploaded()
            )
            .await
            .unwrap(),
        Some(first.clone())
    );
    // A hold confirmed under a claim is still that claim's to take back: what
    // an upload does when making it usable failed partway.
    assert_eq!(
        store.discard(&first, &earlier).await,
        Ok(Discard::Discarded)
    );
    assert!(!holds(&store, &first).await);
    assert!(store
        .read(digest_of(b"same result"), 16)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_release_takes_pending_holds_too_and_a_late_claim_cannot_bring_one_back() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let kept = upload(&store, CONVERSATION, b"kept").await;
    let waiting = hold_for("org", CONVERSATION, b"waiting", b"waiting");
    let claimed = claim(&store, &waiting, b"waiting").await;

    let mut report = store
        .release(&organization("org"), &conversation(CONVERSATION))
        .await
        .unwrap();
    report
        .released
        .sort_by_key(|released| released.hold.stored().size());
    assert_eq!(
        report.released,
        [
            ReleasedHold {
                hold: kept,
                was: HoldState::Held
            },
            ReleasedHold {
                hold: waiting.clone(),
                was: HoldState::Pending
            },
        ]
    );
    assert_eq!(report.removed.len(), 2);
    assert_eq!(report.failures, 0);
    // The upload that was waiting on its evidence finds its hold gone, and
    // neither confirming nor taking back recreates anything.
    assert_eq!(
        store.confirm(&waiting, &claimed).await,
        Ok(Confirmation::Gone)
    );
    assert_eq!(
        store.discard(&waiting, &claimed).await,
        Ok(Discard::NotMine)
    );
    assert!(files_beneath(&root.path().join("attachments/holds")).is_empty());
    assert!(files_beneath(&root.path().join("attachments/blobs")).is_empty());
}

#[tokio::test]
async fn the_same_bytes_kept_as_two_types_are_two_holds_and_one_copy() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let ticket = |media_type: &str| {
        UploadTicket::new(
            organization("org"),
            conversation(CONVERSATION),
            attachment(b"bytes", media_type),
            Caller::new(principal("owner"), "panel", "begin-1").unwrap(),
            TicketLifetime::starting(1_000).unwrap(),
        )
    };
    let png =
        Hold::from_upload(&ticket("image/png"), attachment(b"bytes", "image/png"), 1).unwrap();
    let raw = Hold::from_upload(
        &ticket("application/octet-stream"),
        attachment(b"bytes", "application/octet-stream"),
        2,
    )
    .unwrap();
    keep_usable(&store, &png, b"bytes").await;
    keep_usable(&store, &raw, b"bytes").await;
    // Declaring the bytes again as something else took nothing away.
    assert!(holds(&store, &png).await);
    assert!(holds(&store, &raw).await);
    assert_eq!(
        files_beneath(&root.path().join("attachments/holds")).len(),
        2
    );
    assert_eq!(
        files_beneath(&root.path().join("attachments/blobs")).len(),
        1
    );

    let report = store
        .release(&organization("org"), &conversation(CONVERSATION))
        .await
        .unwrap();
    assert_eq!(report.released.len(), 2);
    // One copy of the bytes, so one removal.
    assert_eq!(report.removed.len(), 1);
    assert_eq!(report.failures, 0);
}

#[tokio::test]
async fn a_pending_hold_left_by_a_crash_stays_invisible_until_replaced_or_released() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let hold = hold_for("org", CONVERSATION, b"bytes", b"bytes");
    let lost = claim(&store, &hold, b"bytes").await;
    drop(store);

    let store = open(root.path());
    assert!(!holds(&store, &hold).await);
    assert_eq!(
        store.read(digest_of(b"bytes"), 16).await.unwrap().unwrap(),
        b"bytes"
    );
    // The same file uploaded again replaces it, under a claim of its own.
    let again = claim(&store, &hold, b"bytes").await;
    assert!(again != lost);
    assert_eq!(
        store.confirm(&hold, &again).await,
        Ok(Confirmation::Confirmed)
    );
    assert!(holds(&store, &hold).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bytes_are_never_lost_while_a_hold_on_them_exists() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(open(root.path()));
    for round in 0..40_u32 {
        let bytes = format!("round {round}").into_bytes();
        let leaving = upload(&store, CONVERSATION, &bytes).await;
        let arriving = hold_for("org", OTHER_CONVERSATION, &bytes, &bytes);
        // The arriving upload is fully staged, so only its publish-and-hold
        // races the release of the last other hold on the same bytes.
        let mut staged = store.stage().await.unwrap();
        staged.write(bytes.clone()).await.unwrap();
        staged.finish().await.unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let keeping = {
            let (barrier, hold, store) = (barrier.clone(), arriving.clone(), store.clone());
            tokio::spawn(async move {
                barrier.wait().await;
                // Its evidence is committed at once, so it confirms at once.
                match staged.keep(hold.clone()).await.unwrap() {
                    Kept::Pending(claim) => assert_eq!(
                        store.confirm(&hold, &claim).await,
                        Ok(Confirmation::Confirmed)
                    ),
                    Kept::Existing(existing) => panic!("already kept: {existing:?}"),
                }
            })
        };
        let releasing = {
            let (barrier, store) = (barrier.clone(), store.clone());
            tokio::spawn(async move {
                barrier.wait().await;
                store
                    .release(&organization("org"), &conversation(CONVERSATION))
                    .await
                    .unwrap()
            })
        };
        let asking = {
            let (barrier, store, hold) = (barrier.clone(), store.clone(), arriving.clone());
            tokio::spawn(async move {
                barrier.wait().await;
                // Whatever this sees, it must be able to see it: never an error
                // from a half-published file.
                holds(&store, &hold).await
            })
        };
        keeping.await.unwrap();
        let report = releasing.await.unwrap();
        asking.await.unwrap();
        assert_eq!(report.failures, 0);
        assert!(!holds(&store, &leaving).await);
        // Whichever went first, the surviving hold still has its bytes.
        assert!(holds(&store, &arriving).await, "round {round}");
        assert_eq!(
            store.read(digest_of(&bytes), 64).await.unwrap().unwrap(),
            bytes
        );
        store
            .release(&organization("org"), &conversation(OTHER_CONVERSATION))
            .await
            .unwrap();
        assert!(store.read(digest_of(&bytes), 64).await.unwrap().is_none());
    }
}
