use super::*;
use crate::domain::common::value_objects::{ImageMediaType, MediaType, Sha256Digest};

#[test]
fn delivery_preserves_original_input_and_appends_confirmed_paths_in_order() {
    let upload = UploadedFileReference::new(
        Sha256Digest::from_bytes([1; 32]),
        MediaType::parse("application/pdf").unwrap(),
        12,
        "report.pdf".into(),
    )
    .unwrap();
    let notes = UploadedFileReference::new(
        Sha256Digest::from_bytes([3; 32]),
        MediaType::parse("text/plain").unwrap(),
        1,
        "notes.txt".into(),
    )
    .unwrap();
    let linked = LinkedFile::new("/workspace/existing.txt".into()).unwrap();
    let image =
        ImageReference::new(Sha256Digest::from_bytes([2; 32]), ImageMediaType::Png, 1).unwrap();
    let original = UserMessage::new(
        None,
        vec![image.clone()],
        vec![linked.clone()],
        vec![upload.clone(), notes.clone()],
    )
    .unwrap();
    let snapshot = original.clone();
    let first = LinkedFile::new("/workspace/delivered-report.pdf".into()).unwrap();
    let second = LinkedFile::new("/workspace/delivered-notes.txt".into()).unwrap();
    let delivered = original
        .clone()
        .delivered_uploads(vec![first.clone(), second.clone()]);
    assert_eq!(original, snapshot);
    assert_eq!(original.uploads(), &[upload, notes]);
    assert!(delivered.uploads().is_empty());
    assert_eq!(delivered.files(), &[linked, first, second]);
    assert_eq!(delivered.images(), &[image]);
    assert_eq!(delivered.text(), original.text());
    assert_eq!(delivered.sender(), original.sender());
    assert_eq!(delivered.clone().delivered_uploads(vec![]), delivered);
}
