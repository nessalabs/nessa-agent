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
        &DirectoryVerificationSource::open(fixture.bundle.clone())?,
        &fixture.key,
        VerificationLimits::default(),
    )
}

#[tokio::test]
async fn an_empty_published_directory_verifies() {
    let fixture = Fixture::empty().await;
    let report = verify(&fixture).unwrap();
    assert_eq!(report.public_records_verified, 0);
    assert_eq!(report.body_bytes_verified, 0);
    assert!(report.omissions.is_empty());
    assert!(!fixture.bundle.join("records").exists());
}

#[tokio::test]
async fn a_published_directory_verifies_with_its_explicit_private_key() {
    let fixture = Fixture::published().await;
    let key = read_verification_key(&fixture.key_file).unwrap();
    let report = verify_bundle(
        &DirectoryVerificationSource::open(fixture.bundle.clone()).unwrap(),
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
        &DirectoryVerificationSource::open(fixture.bundle.clone()).unwrap(),
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

    #[derive(Clone, Copy, Debug)]
    enum RootChange {
        Unchanged,
        Symlink,
        SharedDirectory,
        PrivateDirectory,
        Missing,
        UnsafeMode,
    }

    struct ChangeRootAfterAuthentication<'a> {
        fixture: &'a Fixture,
        change: RootChange,
    }

    impl crate::backup::application::ManifestMacVerifier for ChangeRootAfterAuthentication<'_> {
        fn verify(
            &self,
            canonical: &str,
            mac: crate::backup::domain::Fingerprint,
        ) -> Result<(), VerificationFailure> {
            self.fixture.key.verify(canonical, mac)?;
            if matches!(self.change, RootChange::Unchanged) {
                return Ok(());
            }
            if matches!(self.change, RootChange::UnsafeMode) {
                fs::set_permissions(&self.fixture.bundle, fs::Permissions::from_mode(0o777))
                    .unwrap();
                return Ok(());
            }
            let manifest = fs::read(self.fixture.bundle.join("manifest.txt")).unwrap();
            fs::rename(
                &self.fixture.bundle,
                self.fixture.parent.path().join("detached-root"),
            )
            .unwrap();
            match self.change {
                RootChange::Symlink => {
                    let replacement = self.fixture.parent.path().join("replacement-root");
                    create_directory(&replacement).unwrap();
                    private_file(&replacement.join("manifest.txt"), &manifest);
                    symlink(&replacement, &self.fixture.bundle).unwrap();
                }
                RootChange::SharedDirectory | RootChange::PrivateDirectory => {
                    create_directory(&self.fixture.bundle).unwrap();
                    private_file(&self.fixture.bundle.join("manifest.txt"), &manifest);
                    if matches!(self.change, RootChange::SharedDirectory) {
                        fs::set_permissions(
                            &self.fixture.bundle,
                            fs::Permissions::from_mode(0o777),
                        )
                        .unwrap();
                    }
                }
                RootChange::Missing => {}
                RootChange::Unchanged | RootChange::UnsafeMode => unreachable!(),
            }
            Ok(())
        }
    }

    async fn empty_cut_after_root_change(change: RootChange) {
        let fixture = Fixture::empty().await;
        let manifest = fs::read(fixture.bundle.join("manifest.txt")).unwrap();
        let source = DirectoryVerificationSource::open(fixture.bundle.clone()).unwrap();
        let verifier = ChangeRootAfterAuthentication {
            fixture: &fixture,
            change,
        };
        let result = verify_bundle(&source, &verifier, VerificationLimits::default());
        if matches!(change, RootChange::Unchanged) {
            let report = result.unwrap();
            assert_eq!(report.public_records_verified, 0);
            assert_eq!(report.body_bytes_verified, 0);
            assert_eq!(
                fs::read(fixture.bundle.join("manifest.txt")).unwrap(),
                manifest
            );
        } else {
            assert!(
                matches!(
                    result,
                    Err(VerificationFailure::UnsafeObject | VerificationFailure::Unreadable)
                ),
                "root change {change:?} returned {result:?}"
            );
            let original = if matches!(change, RootChange::UnsafeMode) {
                fixture.bundle.clone()
            } else {
                fixture.parent.path().join("detached-root")
            };
            assert_eq!(fs::read(original.join("manifest.txt")).unwrap(), manifest);
            if matches!(change, RootChange::Missing) {
                assert!(!fixture.bundle.exists());
            } else {
                assert_eq!(
                    fs::read(fixture.bundle.join("manifest.txt")).unwrap(),
                    manifest
                );
                if matches!(change, RootChange::Symlink) {
                    assert!(fs::symlink_metadata(&fixture.bundle)
                        .unwrap()
                        .file_type()
                        .is_symlink());
                } else {
                    let mode = fs::metadata(&fixture.bundle).unwrap().permissions().mode() & 0o777;
                    assert_eq!(
                        mode,
                        if matches!(change, RootChange::SharedDirectory | RootChange::UnsafeMode) {
                            0o777
                        } else {
                            0o700
                        }
                    );
                }
            }
        }
        assert_eq!(fs::read(&fixture.key_file).unwrap(), fixture.key.0);
    }

    #[tokio::test]
    async fn an_empty_cut_with_unchanged_root_verifies() {
        empty_cut_after_root_change(RootChange::Unchanged).await;
    }

    #[tokio::test]
    async fn an_empty_cut_root_symlink_after_authentication_is_refused() {
        empty_cut_after_root_change(RootChange::Symlink).await;
    }

    #[tokio::test]
    async fn an_empty_cut_shared_root_after_authentication_is_refused() {
        empty_cut_after_root_change(RootChange::SharedDirectory).await;
    }

    #[tokio::test]
    async fn an_empty_cut_private_root_replacement_after_authentication_is_refused() {
        empty_cut_after_root_change(RootChange::PrivateDirectory).await;
    }

    #[tokio::test]
    async fn an_empty_cut_missing_root_after_authentication_is_refused() {
        empty_cut_after_root_change(RootChange::Missing).await;
    }

    #[tokio::test]
    async fn an_empty_cut_unsafe_root_mode_after_authentication_is_refused() {
        empty_cut_after_root_change(RootChange::UnsafeMode).await;
    }

    struct ReplaceRootDuringVisit<'a> {
        source: DirectoryVerificationSource,
        fixture: &'a Fixture,
    }

    impl VerificationSource for ReplaceRootDuringVisit<'_> {
        fn manifest(&self) -> Result<Box<dyn Read + '_>, VerificationFailure> {
            self.source.manifest()
        }

        fn record(
            &self,
            store: StoreKind,
            encoded_identity: &str,
        ) -> Result<Box<dyn Read + '_>, VerificationFailure> {
            self.source.record(store, encoded_identity)
        }

        fn visit_records(
            &self,
            visitor: &mut dyn FnMut(StoreKind, &str) -> Result<(), VerificationFailure>,
        ) -> Result<(), VerificationFailure> {
            let manifest = fs::read(self.fixture.bundle.join("manifest.txt")).unwrap();
            let mut replaced = false;
            self.source.visit_records(&mut |store, identity| {
                visitor(store, identity)?;
                if !replaced {
                    replaced = true;
                    fs::rename(
                        &self.fixture.bundle,
                        self.fixture.parent.path().join("detached-root"),
                    )
                    .unwrap();
                    create_directory(&self.fixture.store()).unwrap();
                    private_file(&self.fixture.bundle.join("manifest.txt"), &manifest);
                    private_file(&self.fixture.body(), BODY);
                }
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn a_root_replacement_during_body_enumeration_is_refused() {
        let fixture = Fixture::published().await;
        let manifest = fs::read(fixture.bundle.join("manifest.txt")).unwrap();
        let source = ReplaceRootDuringVisit {
            source: DirectoryVerificationSource::open(fixture.bundle.clone()).unwrap(),
            fixture: &fixture,
        };
        let result = verify_bundle(&source, &fixture.key, VerificationLimits::default());
        assert!(
            matches!(
                result,
                Err(VerificationFailure::UnsafeObject | VerificationFailure::Unreadable)
            ),
            "root replacement during enumeration returned {result:?}"
        );
        let detached = fixture.parent.path().join("detached-root");
        assert_eq!(fs::read(detached.join("manifest.txt")).unwrap(), manifest);
        assert_eq!(
            fs::read(detached.join("records/conversation-metadata/public-row")).unwrap(),
            BODY
        );
        assert_eq!(
            fs::read(fixture.bundle.join("manifest.txt")).unwrap(),
            manifest
        );
        assert_eq!(fs::read(fixture.body()).unwrap(), BODY);
        assert_eq!(fs::read(&fixture.key_file).unwrap(), fixture.key.0);
    }

    struct ReplaceManifestAfterOpen<'a> {
        source: DirectoryVerificationSource,
        fixture: &'a Fixture,
    }

    impl VerificationSource for ReplaceManifestAfterOpen<'_> {
        fn manifest(&self) -> Result<Box<dyn Read + '_>, VerificationFailure> {
            let mut reader = self.source.manifest()?;
            let mut first = [0];
            reader.read_exact(&mut first).unwrap();
            let path = self.fixture.bundle.join("manifest.txt");
            let bytes = fs::read(&path).unwrap();
            assert_eq!(first[0], bytes[0]);
            fs::rename(&path, self.fixture.parent.path().join("detached-manifest")).unwrap();
            private_file(&path, &bytes);
            Ok(Box::new(Cursor::new(first).chain(reader)))
        }

        fn record(
            &self,
            store: StoreKind,
            encoded_identity: &str,
        ) -> Result<Box<dyn Read + '_>, VerificationFailure> {
            self.source.record(store, encoded_identity)
        }

        fn visit_records(
            &self,
            visitor: &mut dyn FnMut(StoreKind, &str) -> Result<(), VerificationFailure>,
        ) -> Result<(), VerificationFailure> {
            self.source.visit_records(visitor)
        }
    }

    #[tokio::test]
    async fn an_identical_manifest_replacement_after_open_is_refused() {
        let fixture = Fixture::published().await;
        let manifest = fs::read(fixture.bundle.join("manifest.txt")).unwrap();
        let source = ReplaceManifestAfterOpen {
            source: DirectoryVerificationSource::open(fixture.bundle.clone()).unwrap(),
            fixture: &fixture,
        };
        assert_eq!(
            verify_bundle(&source, &fixture.key, VerificationLimits::default()).unwrap_err(),
            VerificationFailure::Unreadable
        );
        assert_eq!(
            fs::read(fixture.bundle.join("manifest.txt")).unwrap(),
            manifest
        );
        assert_eq!(
            fs::read(fixture.parent.path().join("detached-manifest")).unwrap(),
            manifest
        );
        assert_eq!(fs::read(fixture.body()).unwrap(), BODY);
        assert_eq!(fs::read(&fixture.key_file).unwrap(), fixture.key.0);
    }

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
            source: DirectoryVerificationSource::open(fixture.bundle.clone()).unwrap(),
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
