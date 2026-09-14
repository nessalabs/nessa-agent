//! Filesystem spelling preserves exact session identities on every supported volume.
use super::{assert_same, id, snapshot, LocalFileStorage, SessionId, SessionStorage, StorageError};
use data_encoding::BASE32HEX_NOPAD;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[tokio::test]
async fn case_distinct_sessions_have_independent_leases_and_durable_histories() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let first_storage = LocalFileStorage::new(&directory).unwrap();
    let second_storage = LocalFileStorage::new(&directory).unwrap();
    let upper = first_storage.open(id("Chat")).await.unwrap();
    let lower = second_storage.open(id("chat")).await.unwrap();
    assert!(matches!(
        second_storage.open(id("Chat")).await,
        Err(StorageError::Busy)
    ));
    assert!(matches!(
        first_storage.open(id("chat")).await,
        Err(StorageError::Busy)
    ));
    let upper_value = snapshot("Chat");
    let lower_value = snapshot("chat");
    upper.save(upper_value.clone()).await.unwrap();
    lower.save(lower_value.clone()).await.unwrap();
    drop(upper);
    drop(lower);
    drop(first_storage);
    drop(second_storage);
    let restored = LocalFileStorage::new(&directory).unwrap();
    let upper = restored.open(id("Chat")).await.unwrap();
    let lower = restored.open(id("chat")).await.unwrap();
    assert_same(&upper.load().await.unwrap().unwrap(), &upper_value);
    assert_same(&lower.load().await.unwrap().unwrap(), &lower_value);
}

// Corruption fixtures address the adapter's documented canonical representation.
// The tests below independently decode actual filenames and check known vectors.
pub(super) fn journal_path(root: &Path, id: &str) -> PathBuf {
    root.join(format!(
        "s-{}.jsonl",
        BASE32HEX_NOPAD.encode(id.as_bytes()).to_ascii_lowercase()
    ))
}
#[cfg(unix)]
pub(super) fn lock_path(root: &Path, id: &str) -> PathBuf {
    journal_path(root, id).with_extension("lock")
}

#[tokio::test]
async fn canonical_file_names_are_case_fold_safe_and_fit_portable_component_limits() {
    let root = tempfile::tempdir().unwrap();
    // Let the adapter create its private directory, including its Windows ACL.
    let directory = root.path().join("private");
    let storage = LocalFileStorage::new(&directory).unwrap();
    let mut identities: Vec<String> = [
        "Chat", "chat", "CON", "con", "PRN", "AUX", "NUL", "COM1", "LPT9",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    identities.extend(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-"
            .chars()
            .map(|character| character.to_string()),
    );
    identities.extend([
        "Z".repeat(SessionId::MAX_BYTES),
        "z".repeat(SessionId::MAX_BYTES),
        "_-Aa09".repeat(21) + "_-",
    ]);
    for identity in &identities {
        let lease = storage.open(id(identity)).await.unwrap();
        lease.save(snapshot(identity)).await.unwrap();
    }
    let mut folded_names = HashSet::new();
    let mut decoded_files = HashSet::new();
    let mut longest = 0;
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.is_ascii());
        assert!(name.len() <= 213);
        assert!(
            folded_names.insert(name.to_ascii_lowercase()),
            "case-fold collision: {name}"
        );
        longest = longest.max(name.len());
        let encoded = path
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .strip_prefix("s-")
            .unwrap();
        assert!(encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'v').contains(&byte)));
        let decoded = String::from_utf8(
            BASE32HEX_NOPAD
                .decode(encoded.to_ascii_uppercase().as_bytes())
                .unwrap(),
        )
        .unwrap();
        assert!(identities.contains(&decoded));
        let extension = path.extension().unwrap().to_str().unwrap();
        assert!(matches!(extension, "jsonl" | "lock"));
        assert!(decoded_files.insert((decoded, extension.to_owned())));
    }
    assert_eq!(longest, 213);
    assert_eq!(decoded_files.len(), identities.len() * 2);
    assert!(directory.join("s-8dk62t0.jsonl").is_file());
    assert!(directory.join("s-cdk62t0.jsonl").is_file());
    for identity in identities {
        let lease = storage.open(id(&identity)).await.unwrap();
        assert_same(&lease.load().await.unwrap().unwrap(), &snapshot(&identity));
    }
    for invalid in [".", "..", "CON.", "Chat ", "é", "e\u{301}", "a/b", "a\\b"] {
        assert!(SessionId::new(invalid).is_err());
    }
}
