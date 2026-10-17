//! The real private-directory reader against authentic synthetic publications.

use std::fs;

use crate as server;
use crate::backup::application::{
    verify_bundle, InventoryVerification, MacKey, VerificationFailure, VerificationLimits,
    VerificationReport,
};
use crate::backup::infrastructure::{read_verification_key, DirectoryVerificationSource};

#[path = "verification_fixture.rs"]
mod fixture;
use fixture::{private_file, Fixture, BODY};

fn verify(fixture: &Fixture) -> Result<VerificationReport, VerificationFailure> {
    verify_bundle(
        &DirectoryVerificationSource::new(fixture.bundle.clone()),
        &fixture.key,
        VerificationLimits::default(),
    )
}

#[tokio::test]
async fn a_published_directory_verifies_with_its_explicit_private_key() {
    let fixture = Fixture::published().await;
    let key = read_verification_key(&fixture.key_file).unwrap();
    let report = verify_bundle(
        &DirectoryVerificationSource::new(fixture.bundle.clone()),
        &key,
        VerificationLimits::default(),
    )
    .unwrap();
    assert_eq!(report.public_records_verified, 1);
    assert_eq!(report.body_bytes_verified, BODY.len() as u64);
    assert!(matches!(
        report.inventory,
        InventoryVerification::NotIncluded
    ));
    assert_eq!(report.omissions.len(), 1);
    assert_eq!(report.omissions[0].identity, "redacted-row");
    assert_eq!(report.omissions[0].kind, "redacted");
    assert_eq!(report.restore_readiness, "not_assessed");
    assert_eq!(report.inventory_freshness, "not_assessed");
}

#[tokio::test]
async fn corrupted_and_truncated_directory_bodies_are_refused() {
    let fixture = Fixture::published().await;
    for bytes in [b"tampered".as_slice(), &BODY[..BODY.len() - 1], b""] {
        fs::write(fixture.body(), bytes).unwrap();
        assert_eq!(
            verify(&fixture).unwrap_err(),
            VerificationFailure::BodyFingerprint
        );
    }
}

#[tokio::test]
async fn a_wrong_key_refuses_an_authentic_directory() {
    let fixture = Fixture::published().await;
    let result = verify_bundle(
        &DirectoryVerificationSource::new(fixture.bundle.clone()),
        &MacKey([38; 32]),
        VerificationLimits::default(),
    );
    assert_eq!(result.unwrap_err(), VerificationFailure::Authentication);
}

#[tokio::test]
async fn missing_extra_and_leaked_redacted_bodies_are_refused() {
    for name in ["extra-row", "redacted-row"] {
        let fixture = Fixture::published().await;
        private_file(&fixture.store().join(name), b"SYNTHETIC-UNLISTED");
        assert_eq!(
            verify(&fixture).unwrap_err(),
            VerificationFailure::UnexpectedRecord
        );
    }
    let fixture = Fixture::published().await;
    fs::remove_file(fixture.body()).unwrap();
    assert_eq!(
        verify(&fixture).unwrap_err(),
        VerificationFailure::MissingRecord
    );
    fs::remove_dir_all(fixture.bundle.join("records")).unwrap();
    assert_eq!(
        verify(&fixture).unwrap_err(),
        VerificationFailure::MissingRecord
    );
}

#[tokio::test]
async fn unknown_directory_layout_is_refused() {
    let fixture = Fixture::published().await;
    private_file(&fixture.bundle.join("unexpected.txt"), b"synthetic");
    assert_eq!(
        verify(&fixture).unwrap_err(),
        VerificationFailure::InvalidLayout
    );
}

#[test]
fn key_reads_do_not_create_or_repair_missing_short_or_oversized_keys() {
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("absent-parent/key");
    assert_eq!(
        read_verification_key(&missing),
        Err(VerificationFailure::Unreadable)
    );
    assert!(!missing.parent().unwrap().exists());
    for size in [0, 1, 31, 33, 8192] {
        let path = parent.path().join(format!("key-{size}"));
        let bytes = vec![73; size];
        private_file(&path, &bytes);
        assert_eq!(
            read_verification_key(&path),
            Err(VerificationFailure::InvalidKey)
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[cfg(unix)]
mod unix {
    use std::io::{Cursor, Read};
    use std::os::unix::fs::{symlink, PermissionsExt};

    use nessa_local_storage::create_directory;

    use crate::backup::application::VerificationSource;
    use crate::backup::domain::StoreKind;

    use super::*;

    #[derive(Clone, Copy)]
    enum Replacement {
        Body,
        Store,
    }

    struct ReplaceAfterOpen<'a> {
        source: DirectoryVerificationSource,
        fixture: &'a Fixture,
        replacement: Replacement,
    }

    impl VerificationSource for ReplaceAfterOpen<'_> {
        fn manifest(&self) -> Result<Box<dyn Read + '_>, VerificationFailure> {
            self.source.manifest()
        }

        fn record(
            &self,
            store: StoreKind,
            encoded_identity: &str,
        ) -> Result<Box<dyn Read + '_>, VerificationFailure> {
            let mut reader = self.source.record(store, encoded_identity)?;
            let mut first = [0];
            reader.read_exact(&mut first).unwrap();
            assert_eq!(first[0], BODY[0]);

            let original = self.fixture.parent.path().join("detached-original");
            match self.replacement {
                Replacement::Body => {
                    fs::rename(self.fixture.body(), original).unwrap();
                }
                Replacement::Store => {
                    fs::rename(self.fixture.store(), original).unwrap();
                    create_directory(&self.fixture.store()).unwrap();
                }
            }
            private_file(&self.fixture.body(), BODY);

            // Replay the consumed byte so the fingerprint sees the authentic body.
            // Only the real reader's EOF binding check can detect this replacement.
            Ok(Box::new(Cursor::new(first).chain(reader)))
        }

        fn visit_records(
            &self,
            visitor: &mut dyn FnMut(StoreKind, &str) -> Result<(), VerificationFailure>,
        ) -> Result<(), VerificationFailure> {
            self.source.visit_records(visitor)
        }
    }

    async fn replacement_after_open_is_refused(replacement: Replacement) {
        let fixture = Fixture::published().await;
        let source = ReplaceAfterOpen {
            source: DirectoryVerificationSource::new(fixture.bundle.clone()),
            fixture: &fixture,
            replacement,
        };
        assert_eq!(
            verify_bundle(&source, &fixture.key, VerificationLimits::default()).unwrap_err(),
            VerificationFailure::Unreadable
        );
        assert_eq!(fs::read(fixture.body()).unwrap(), BODY);
        let original = fixture.parent.path().join("detached-original");
        let original_body = match replacement {
            Replacement::Body => original,
            Replacement::Store => original.join("public-row"),
        };
        assert_eq!(fs::read(original_body).unwrap(), BODY);
        // A new reader can verify the replacement; the failed reader did not repair it.
        assert!(verify(&fixture).is_ok());
    }

    #[tokio::test]
    async fn a_body_replacement_after_open_is_refused() {
        replacement_after_open_is_refused(Replacement::Body).await;
    }

    #[tokio::test]
    async fn a_store_replacement_after_open_is_refused() {
        replacement_after_open_is_refused(Replacement::Store).await;
    }

    #[tokio::test]
    async fn symlinks_at_each_bundle_boundary_are_refused() {
        for relative in [
            "",
            "manifest.txt",
            "records",
            "records/conversation-metadata",
            "records/conversation-metadata/public-row",
        ] {
            let fixture = Fixture::published().await;
            let path = if relative.is_empty() {
                fixture.bundle.clone()
            } else {
                fixture.bundle.join(relative)
            };
            let detached = fixture.parent.path().join("detached-original");
            fs::rename(&path, &detached).unwrap();
            symlink(&detached, &path).unwrap();
            assert!(
                matches!(
                    verify(&fixture),
                    Err(VerificationFailure::UnsafeObject | VerificationFailure::Unreadable)
                ),
                "symlink accepted at {relative}"
            );
        }
    }

    #[tokio::test]
    async fn hardlinked_manifest_body_and_key_are_refused() {
        for relative in ["manifest.txt", "records/conversation-metadata/public-row"] {
            let fixture = Fixture::published().await;
            fs::hard_link(
                fixture.bundle.join(relative),
                fixture.parent.path().join("alias"),
            )
            .unwrap();
            assert_eq!(
                verify(&fixture).unwrap_err(),
                VerificationFailure::UnsafeObject
            );
        }
        let fixture = Fixture::published().await;
        fs::hard_link(&fixture.key_file, fixture.parent.path().join("key-alias")).unwrap();
        assert_eq!(
            read_verification_key(&fixture.key_file),
            Err(VerificationFailure::UnsafeObject)
        );
    }

    #[tokio::test]
    async fn a_symlink_key_is_refused_without_modifying_its_target() {
        let fixture = Fixture::published().await;
        let link = fixture.parent.path().join("key-link");
        symlink(&fixture.key_file, &link).unwrap();
        assert!(matches!(
            read_verification_key(&link),
            Err(VerificationFailure::UnsafeObject | VerificationFailure::Unreadable)
        ));
        assert_eq!(fs::read(&fixture.key_file).unwrap(), fixture.key.0);
    }

    #[tokio::test]
    async fn unsafe_permissions_are_refused_without_repair() {
        for relative in [
            "",
            "manifest.txt",
            "records",
            "records/conversation-metadata",
            "records/conversation-metadata/public-row",
        ] {
            let fixture = Fixture::published().await;
            let path = if relative.is_empty() {
                fixture.bundle.clone()
            } else {
                fixture.bundle.join(relative)
            };
            fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
            assert_eq!(
                verify(&fixture).unwrap_err(),
                VerificationFailure::UnsafeObject
            );
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o777
            );
        }
        let fixture = Fixture::published().await;
        fs::set_permissions(&fixture.key_file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            read_verification_key(&fixture.key_file),
            Err(VerificationFailure::UnsafeObject)
        );
        assert_eq!(
            fs::metadata(&fixture.key_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }
}
