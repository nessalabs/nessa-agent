//! Private command configuration uses the real file and private-state owners.
use super::{Profile, ProfileError, MAX_PROFILE_BYTES};
use nessa_auth::application::pairing::ClientPendingStore;
use nessa_local_storage::OpenMode;
use serde_json::json;
#[cfg(unix)]
use std::fs::Permissions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn write(path: &Path, bytes: &[u8]) {
    nessa_local_storage::open(path, OpenMode::CreateNew)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn document(root: &Path) -> Vec<u8> {
    serde_json::to_vec(&json!({"stateRoot": root, "stateDirectory": "device",
        "cache": root.join("cache.sqlite3"), "gatewayAddress": "127.0.0.1:47650"}))
    .unwrap()
}

/// Profile admission: bounded, strict, a numeric address, an absolute root and
/// a relative directory; parsing touches no private state.
#[test]
fn private_profile_admission_is_bounded_and_explicit() {
    let root = tempfile::tempdir().unwrap();
    let valid = document(root.path());
    let path = root.path().join("valid.json");
    let mut exact = valid.clone();
    exact.resize(MAX_PROFILE_BYTES, b' ');
    write(&path, &exact);
    let profile = Profile::load(&path).unwrap();
    assert_eq!(profile.gateway, "127.0.0.1:47650".parse().unwrap());
    assert_eq!(profile.cache, root.path().join("cache.sqlite3"));
    let oversized = root.path().join("oversized.json");
    exact.push(b' ');
    write(&oversized, &exact);
    assert!(matches!(
        Profile::load(&oversized),
        Err(ProfileError::TooLarge)
    ));
    assert!(matches!(
        Profile::load(&root.path().join("missing")),
        Err(ProfileError::Unavailable)
    ));
    let text = String::from_utf8(valid).unwrap();
    for (index, text) in [
        "{broken".to_owned(),
        text.replace("127.0.0.1:47650", "localhost:47650"),
        text.replace("127.0.0.1:47650", "127.0.0.1"),
        text.replace("\"device\"", "\"device\",\"unknown\":true"),
        text.replace("\"device\"", "\"device\",\"stateDirectory\":\"other\""),
        text.replace("\"device\"", &serde_json::to_string(root.path()).unwrap()),
        text.replace(
            &serde_json::to_string(root.path()).unwrap(),
            "\"relative-root\"",
        ),
        text.replace(
            &serde_json::to_string(&root.path().join("cache.sqlite3")).unwrap(),
            "\"relative.sqlite3\"",
        ),
        // The bearer profile is not a current shape.
        serde_json::to_string(&json!({"receiver":"r","accessEpoch":1,
            "credentialFile":root.path(),"endpointRoot":root.path(),
            "endpointDirectory":"gateway"}))
        .unwrap(),
    ]
    .into_iter()
    .enumerate()
    {
        let path = root.path().join(format!("invalid-{index}.json"));
        write(&path, text.as_bytes());
        assert!(
            matches!(Profile::load(&path), Err(ProfileError::Invalid)),
            "case {index}"
        );
    }
    assert!(!root.path().join("device").exists());
}

/// The private state directory is created beneath the root on first use and
/// opened through its owner: empty, and exclusive to one opener.
#[test]
fn profile_opens_one_private_state_owner() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("private");
    nessa_local_storage::create_directory(&root).unwrap();
    let path = root.join("profile.json");
    write(&path, &document(&root));
    let profile = Profile::load(&path).unwrap();
    let state = profile.private_state().unwrap();
    assert!(state.load_credential().unwrap().is_none());
    assert!(state.load_pending().unwrap().is_none());
    assert_eq!(
        profile.private_state().err(),
        Some(ProfileError::PrivateStateUnavailable),
        "a second opener is refused while the first holds it"
    );
    drop(state);
    assert!(profile.private_state().is_ok());
}

#[cfg(unix)]
#[test]
fn profile_requires_a_private_file() {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("profile.json");
    write(&private, &document(root.path()));
    assert!(Profile::load(&private).is_ok());
    std::fs::set_permissions(&private, Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        Profile::load(&private),
        Err(ProfileError::Unavailable)
    ));
}
