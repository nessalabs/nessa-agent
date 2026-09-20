use super::super::generation::random_generation;
use super::{
    clone_file, entry_name, finalize_file, launch_settings, publish, stage_runtime,
    stage_runtime_using, tree_fingerprint, utf8, validate_runtime,
};
use serde_json::json;
use std::{
    ffi::{CString, OsString},
    fs::{self, OpenOptions, Permissions},
    os::unix::{
        ffi::OsStringExt,
        fs::{symlink, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
};

const TEST_ATTRIBUTE: &[u8] = b"com.nessa.runtime-staging-test\0";

fn set_test_attribute(path: &Path) {
    let path = CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(
        unsafe {
            libc::setxattr(
                path.as_ptr(),
                TEST_ATTRIBUTE.as_ptr().cast(),
                b"untrusted".as_ptr().cast(),
                b"untrusted".len(),
                0,
                0,
            )
        },
        0
    );
}

fn has_test_attribute(path: &Path) -> bool {
    let path = CString::new(path.to_str().unwrap()).unwrap();
    let result = unsafe {
        libc::getxattr(
            path.as_ptr(),
            TEST_ATTRIBUTE.as_ptr().cast(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if result >= 0 {
        true
    } else {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOATTR)
        );
        false
    }
}

fn force_byte_copy(_: &Path, _: &Path) -> Result<bool, String> {
    Ok(false)
}

fn fail_clone_attempt(_: &Path, _: &Path) -> Result<bool, String> {
    Err("forced clone failure".into())
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "nessa-runtime-stage-{}",
            random_generation().unwrap()
        ));
        nessa_local_storage::create_directory(&path).unwrap();
        Self(path)
    }
    fn source(&self) -> PathBuf {
        let source = self.0.join("source");
        nessa_local_storage::create_directory(&source).unwrap();
        fs::write(source.join("nessa"), b"gateway\0bytes").unwrap();
        fs::set_permissions(source.join("nessa"), Permissions::from_mode(0o751)).unwrap();
        fs::write(source.join("node"), b"node").unwrap();
        source
    }
    fn manifest(source: &Path) -> String {
        let fingerprint = tree_fingerprint(source).unwrap();
        fs::write(
            source.join("manifest.json"),
            serde_json::to_vec(&json!({"fingerprint":fingerprint})).unwrap(),
        )
        .unwrap();
        fingerprint
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn rust_fingerprint_matches_javascript_with_unicode_and_escape_framing() {
    let fixture = Fixture::new();
    let source = fixture.source();
    for name in ["quote\"\\\n\u{2028}.txt", "\u{e000}", "\u{10000}", "é", "𝄞"] {
        fs::write(source.join(name), name.as_bytes()).unwrap();
    }
    fs::create_dir(source.join("nested")).unwrap();
    fs::write(source.join("nested/manifest.json"), b"included").unwrap();
    symlink("quote\"\\\n\u{2028}.txt", source.join("link")).unwrap();
    let expected = Fixture::manifest(&source);
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/desktop/runtime-fingerprint.mjs");
    let result=Command::new("node").args(["--input-type=module","-e","const {runtimeFingerprint}=await import(process.argv[1]); process.stdout.write(runtimeFingerprint(process.argv[2]));"]).arg(script).arg(&source).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8(result.stdout).unwrap(), expected);
    let staged = stage_runtime(&source, &fixture.0.join("versions"), &expected).unwrap();
    assert_eq!(tree_fingerprint(&staged).unwrap(), expected);
}
#[test]
fn published_runtime_is_private_reusable_immutable_and_definition_uses_staged_paths() {
    let fixture = Fixture::new();
    let source = fixture.source();
    symlink("nessa", source.join("gateway-link")).unwrap();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let staged = stage_runtime(&source, &versions, &fingerprint).unwrap();
    assert_eq!(staged, versions.join(&fingerprint));
    assert_eq!(
        fs::metadata(&staged).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(staged.join("nessa"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o711
    );
    assert_eq!(fs::read(staged.join("nessa")).unwrap(), b"gateway\0bytes");
    assert_eq!(
        fs::read_link(staged.join("gateway-link")).unwrap(),
        Path::new("nessa")
    );
    // The service runs the staged copy, never the bundle it was staged from,
    // and it is addressed absolutely: no search path is derived from it.
    assert_eq!(
        launch_settings(&staged),
        json!([staged.join("nessa"), "server", "--desktop-runtime", staged])
    );
    assert!(!launch_settings(&staged)
        .to_string()
        .contains(source.to_str().unwrap()));
    assert_eq!(
        stage_runtime(&source, &versions, &fingerprint).unwrap(),
        staged
    );
    fs::write(source.join("nessa"), b"app was replaced").unwrap();
    assert_eq!(
        stage_runtime(&source, &versions, &fingerprint).unwrap(),
        staged
    );
    assert_eq!(fs::read(staged.join("nessa")).unwrap(), b"gateway\0bytes");
    fs::write(staged.join("nessa"), b"corruption").unwrap();
    assert!(stage_runtime(&source, &versions, &fingerprint).is_err());
    assert_eq!(fs::read(staged.join("nessa")).unwrap(), b"corruption");
}

#[test]
fn cloned_files_are_synced_through_the_normalized_finalization_boundary() {
    let fixture = Fixture::new();
    let source = fixture.source().join("node");
    set_test_attribute(&source);
    let cloned = fixture.0.join("cloned-node");
    assert!(clone_file(&source, &cloned).unwrap());
    assert!(has_test_attribute(&cloned));

    let file = OpenOptions::new().write(true).open(&cloned).unwrap();
    finalize_file(&file, 0o755).unwrap();

    assert!(!has_test_attribute(&cloned));
    assert_eq!(
        fs::metadata(&cloned).unwrap().permissions().mode() & 0o777,
        0o711
    );
}

#[test]
fn published_runtime_removes_bundle_extended_attributes() {
    let fixture = Fixture::new();
    let source = fixture.source();
    set_test_attribute(&source.join("nessa"));
    set_test_attribute(&source.join("node"));
    let fingerprint = Fixture::manifest(&source);

    let staged = stage_runtime(&source, &fixture.0.join("versions"), &fingerprint).unwrap();

    assert!(has_test_attribute(&source.join("nessa")));
    assert!(has_test_attribute(&source.join("node")));
    assert!(!has_test_attribute(&staged.join("nessa")));
    assert!(!has_test_attribute(&staged.join("node")));
}

#[test]
fn byte_copy_fallback_preserves_the_published_runtime_contract() {
    let fixture = Fixture::new();
    let source = fixture.source();
    set_test_attribute(&source.join("nessa"));
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");

    let staged = stage_runtime_using(&source, &versions, &fingerprint, force_byte_copy).unwrap();

    assert_eq!(staged, versions.join(&fingerprint));
    assert_eq!(fs::read(staged.join("nessa")).unwrap(), b"gateway\0bytes");
    assert_eq!(
        fs::metadata(staged.join("nessa"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o711
    );
    assert!(has_test_attribute(&source.join("nessa")));
    assert!(!has_test_attribute(&staged.join("nessa")));
    assert_eq!(tree_fingerprint(&staged).unwrap(), fingerprint);
}

#[test]
fn clone_attempt_failure_removes_its_unpublished_runtime() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");

    let error =
        stage_runtime_using(&source, &versions, &fingerprint, fail_clone_attempt).unwrap_err();

    assert_eq!(error, "forced clone failure");
    assert!(!versions.join(&fingerprint).exists());
    assert_eq!(fs::read_dir(versions).unwrap().count(), 0);
}
#[test]
fn mixed_or_interrupted_copy_never_publishes_or_removes_another_attempt() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    nessa_local_storage::create_directory(&versions).unwrap();
    let unrelated = versions.join(".staging-another-attempt");
    fs::create_dir(&unrelated).unwrap();
    fs::write(unrelated.join("keep"), b"keep").unwrap();
    fs::write(source.join("node"), b"mixed new version").unwrap();
    assert!(stage_runtime(&source, &versions, &fingerprint).is_err());
    assert!(!versions.join(&fingerprint).exists());
    assert_eq!(fs::read_dir(&versions).unwrap().count(), 1);
    assert!(unrelated.join("keep").exists());
}
#[test]
fn links_special_entries_and_non_utf8_names_are_rejected() {
    for case in ["absolute", "escape", "broken", "fifo", "manifest-link"] {
        let fixture = Fixture::new();
        let source = fixture.source();
        let fingerprint = Fixture::manifest(&source);
        let external = fixture.0.join("outside");
        fs::write(&external, b"outside").unwrap();
        match case {
            "absolute" => symlink(source.join("node"), source.join("bad")).unwrap(),
            "escape" => symlink("../outside", source.join("bad")).unwrap(),
            "broken" => symlink("missing", source.join("bad")).unwrap(),
            "fifo" => {
                let path = CString::new(source.join("bad").to_str().unwrap()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            "manifest-link" => {
                fs::rename(source.join("manifest.json"), source.join("saved-manifest")).unwrap();
                symlink("saved-manifest", source.join("manifest.json")).unwrap();
            }
            _ => unreachable!(),
        }
        let versions = fixture.0.join("versions");
        assert!(
            stage_runtime(&source, &versions, &fingerprint).is_err(),
            "{case}"
        );
        assert!(!versions.join(&fingerprint).exists());
        assert_eq!(fs::read_dir(versions).unwrap().count(), 0);
    }
}
#[test]
fn published_permissions_and_manifest_are_validated_without_repair() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let staged = stage_runtime(&source, &versions, &fingerprint).unwrap();
    fs::set_permissions(staged.join("node"), Permissions::from_mode(0o644)).unwrap();
    assert!(validate_runtime(&staged, &fingerprint).is_err());
    assert_eq!(
        fs::metadata(staged.join("node"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    fs::set_permissions(staged.join("node"), Permissions::from_mode(0o600)).unwrap();
    fs::write(staged.join("manifest.json"), b"invalid").unwrap();
    assert!(stage_runtime(&source, &versions, &fingerprint).is_err());
}

#[test]
fn non_utf8_representation_is_rejected_even_on_filesystems_that_cannot_create_it() {
    let invalid = OsString::from_vec(vec![0xff]);
    assert!(entry_name(invalid.clone()).is_err());
    assert!(utf8(Path::new(&invalid)).is_err());
}
#[test]
fn retained_versions_and_exclusive_publication_never_replace_existing_directories() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let first = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let original = stage_runtime(&source, &versions, &first).unwrap();
    fs::write(source.join("node"), b"new node").unwrap();
    let second = Fixture::manifest(&source);
    let updated = stage_runtime(&source, &versions, &second).unwrap();
    assert_ne!(original, updated);
    assert_eq!(fs::read(original.join("node")).unwrap(), b"node");
    assert_eq!(fs::read(updated.join("node")).unwrap(), b"new node");
    let temporary = versions.join(".owned-test");
    fs::create_dir(&temporary).unwrap();
    fs::write(temporary.join("keep"), b"keep").unwrap();
    let reserved = versions.join("reserved");
    fs::create_dir(&reserved).unwrap();
    assert!(publish(&temporary, &reserved).is_err());
    assert!(temporary.join("keep").exists());
    assert_eq!(fs::read_dir(reserved).unwrap().count(), 0);
}

#[test]
fn replacing_bundle_after_manifest_selection_cannot_publish_the_wrong_version() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let selected = Fixture::manifest(&source);
    fs::rename(&source, fixture.0.join("previous-bundle")).unwrap();
    let replacement = fixture.source();
    fs::write(replacement.join("nessa"), b"replacement gateway").unwrap();
    assert_ne!(Fixture::manifest(&replacement), selected);
    let versions = fixture.0.join("versions");
    assert!(stage_runtime(&replacement, &versions, &selected).is_err());
    assert!(!versions.join(selected).exists());
    assert_eq!(fs::read_dir(versions).unwrap().count(), 0);
}
