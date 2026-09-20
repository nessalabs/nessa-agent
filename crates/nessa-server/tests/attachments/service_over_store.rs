//! The service over the real store: the interleavings where what is on disk
//! decides who owns a hold's creation, and who may undo it.
use super::*;
use crate::attachments::application::{
    AttachmentAuditRecord, AttachmentLimits, AttachmentStore, BeginOutcome, NormalizeError,
    UploadError,
};
use crate::attachments::domain::Attachment;
use crate::attachments_test_support::{
    attachment, begin_request, caller, conversation, digest_of, organization, ChannelBody, Fixture,
    MemoryStore, StubNormalizer, CONVERSATION,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const PDF: &str = "application/pdf";
const BYTES: &[u8] = b"twenty bytes of file";

fn over_real_store(
    root: &Path,
    normalizer: StubNormalizer,
) -> (Fixture, Arc<LocalAttachmentStore>) {
    let store = Arc::new(LocalAttachmentStore::open(root.join("attachments")).unwrap());
    let fixture = Fixture::over(
        store.clone(),
        Arc::new(MemoryStore::default()),
        AttachmentLimits::default(),
        normalizer,
    );
    (fixture, store)
}
fn records_beneath(directory: PathBuf) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(records_beneath(path));
        } else {
            found.push(path);
        }
    }
    found
}
async fn held(fixture: &Fixture, stored: &Attachment) -> bool {
    fixture
        .service
        .holds(&organization("org"), &conversation(CONVERSATION), stored)
        .await
        .unwrap()
}

#[tokio::test]
async fn taking_back_an_unrecorded_upload_never_destroys_a_later_acknowledged_one() {
    let root = tempfile::tempdir().unwrap();
    let (fixture, store) =
        over_real_store(root.path(), StubNormalizer::failing(NormalizeError::Failed));
    let first = fixture.ticket_as("begin-1", CONVERSATION, BYTES, PDF).await;
    let second = fixture.ticket_as("begin-2", CONVERSATION, BYTES, PDF).await;
    // The first upload's creation record waits at the sink, and is then refused.
    let open = fixture.audit.hold_after(0, true);
    let racing = fixture.service.clone();
    let entered = fixture.audit.entered.notified();
    let one = tokio::spawn(async move {
        racing
            .receive(&first, Some(BYTES.len() as u64), ChannelBody::of(BYTES, 7))
            .await
    });
    entered.await;
    // While its hold is unrecorded, a begin does not answer "stored" for it.
    let mut again = begin_request(CONVERSATION, BYTES, PDF);
    again.request_id = "begin-3".into();
    assert!(matches!(
        fixture.service.begin(caller("org", "owner"), again).await,
        Ok(BeginOutcome::UploadRequired { .. })
    ));
    // The second upload of the same file is recorded and answered 200.
    fixture
        .service
        .receive(&second, Some(BYTES.len() as u64), ChannelBody::of(BYTES, 7))
        .await
        .unwrap();
    open.send(()).unwrap();
    assert_eq!(one.await.unwrap(), Err(UploadError::AuditUnavailable));
    // What was acknowledged is still held, bytes and all.
    assert!(held(&fixture, &attachment(BYTES, PDF)).await);
    assert_eq!(
        store.read(digest_of(BYTES), 1024).await.unwrap().unwrap(),
        BYTES
    );
}

#[tokio::test]
async fn declaring_the_same_bytes_as_something_else_keeps_the_image_a_message_named() {
    let root = tempfile::tempdir().unwrap();
    let png = b"bytes the normalizer passes through";
    let (fixture, _) = over_real_store(root.path(), StubNormalizer::producing(png, "image/png"));
    let image = fixture.upload(CONVERSATION, png, "image/png").await;
    assert!(held(&fixture, &image).await);
    fixture.audit.taken_all();

    let other = fixture
        .upload(CONVERSATION, png, "application/octet-stream")
        .await;
    // Two holds on one copy of the bytes; the first is untouched, and the
    // record of the second says a hold was created, not that one was replaced.
    assert!(held(&fixture, &image).await);
    assert!(held(&fixture, &other).await);
    assert_eq!(
        records_beneath(root.path().join("attachments/holds")).len(),
        2
    );
    assert_eq!(
        records_beneath(root.path().join("attachments/blobs")).len(),
        1
    );
    let records = fixture.audit.taken();
    assert!(
        matches!(
            records.as_slice(),
            [AttachmentAuditRecord::HoldCreated { hold }] if hold.stored() == &other
        ),
        "{records:?}"
    );
}
