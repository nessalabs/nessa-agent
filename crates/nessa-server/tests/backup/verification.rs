//! Application verification through bounded read substitutes, without disk I/O.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::{self, Cursor, Read};
use std::rc::Rc;

use crate::backup::application::{
    verify_bundle, Authenticator, InventoryVerification, MacKey, VerificationFailure,
    VerificationLimits, VerificationSource,
};
use crate::backup::domain::{
    encode_identity, AuthenticatedManifest, CapturedRecord, DeferredEntry, DeletionLog,
    Fingerprint, Manifest, Sensitivity, SkipReason, SkippedRecord, SourceCut, StoreKind, Watermark,
    WRITER_INVENTORY,
};

const KEY: MacKey = MacKey([37; 32]);

struct MemoryBundle {
    manifest: Vec<u8>,
    bodies: BTreeMap<(StoreKind, String), Vec<u8>>,
    payload_access: Cell<usize>,
    manifest_read: Rc<Cell<usize>>,
    body_read: Rc<Cell<usize>>,
    repeat_bodies: bool,
}

impl VerificationSource for MemoryBundle {
    fn manifest(&self) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        Ok(Box::new(Metered {
            reader: Cursor::new(self.manifest.as_slice()),
            count: self.manifest_read.clone(),
        }))
    }

    fn record(
        &self,
        store: StoreKind,
        name: &str,
    ) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        self.payload_access.set(self.payload_access.get() + 1);
        if self.repeat_bodies {
            return Ok(Box::new(Metered {
                reader: io::repeat(b'x'),
                count: self.body_read.clone(),
            }));
        }
        let bytes = self
            .bodies
            .get(&(store, name.to_owned()))
            .ok_or(VerificationFailure::Unreadable)?;
        Ok(Box::new(Metered {
            reader: Cursor::new(bytes.as_slice()),
            count: self.body_read.clone(),
        }))
    }

    fn visit_records(
        &self,
        visitor: &mut dyn FnMut(StoreKind, &str) -> Result<(), VerificationFailure>,
    ) -> Result<(), VerificationFailure> {
        self.payload_access.set(self.payload_access.get() + 1);
        for (store, identity) in self.bodies.keys() {
            visitor(*store, identity)?;
        }
        Ok(())
    }
}

struct Metered<R> {
    reader: R,
    count: Rc<Cell<usize>>,
}
impl<R: Read> Read for Metered<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.reader.read(buffer)?;
        self.count.set(self.count.get() + count);
        Ok(count)
    }
}

fn from_cuts(cuts: Vec<SourceCut>) -> MemoryBundle {
    let manifest = Manifest::new(42, cuts).unwrap();
    let mac = KEY.mac(&manifest.canonical()).unwrap();
    let sealed = AuthenticatedManifest::seal(manifest, mac);
    let mut bodies = BTreeMap::new();
    for store in sealed.manifest().stores() {
        for record in store.records() {
            if let Some(bytes) = record.public_bytes() {
                bodies.insert(
                    (store.store(), encode_identity(record.identity())),
                    bytes.to_vec(),
                );
            }
        }
    }
    MemoryBundle {
        manifest: sealed.encoded().into_bytes(),
        bodies,
        payload_access: Cell::new(0),
        manifest_read: Rc::new(Cell::new(0)),
        body_read: Rc::new(Cell::new(0)),
        repeat_bodies: false,
    }
}

fn public(store: StoreKind, name: &str, bytes: &[u8]) -> CapturedRecord {
    CapturedRecord::new(store, name, Sensitivity::Public, bytes.to_vec()).unwrap()
}

fn bundle(inventory: &str, watermark: &str) -> MemoryBundle {
    from_cuts(
        WRITER_INVENTORY
            .iter()
            .map(|kind| match kind {
                StoreKind::CanonicalRecords => SourceCut::new(
                    *kind,
                    Watermark::new("cut").unwrap(),
                    vec![
                        public(*kind, "first", b"abc"),
                        public(*kind, "second", b"def"),
                    ],
                    vec![],
                )
                .unwrap(),
                StoreKind::DeletionInventory => SourceCut::new(
                    *kind,
                    Watermark::new(watermark).unwrap(),
                    vec![public(*kind, "log", inventory.as_bytes())],
                    vec![],
                )
                .unwrap(),
                _ => SourceCut::absent(*kind).unwrap(),
            })
            .collect(),
    )
}

fn valid() -> MemoryBundle {
    bundle(&DeletionLog::default().canonical(), "0")
}

fn result(bundle: &MemoryBundle, limits: VerificationLimits) -> Result<(), VerificationFailure> {
    verify_bundle(bundle, &KEY, limits).map(|_| ())
}

#[test]
fn authentication_precedes_payload_reads() {
    let source = valid();
    assert_eq!(
        verify_bundle(&source, &MacKey([38; 32]), VerificationLimits::default()).unwrap_err(),
        VerificationFailure::Authentication
    );
    assert_eq!(source.payload_access.get(), 0);
    let mut source = valid();
    let length = source.manifest.len();
    source.manifest[length - 2] = if source.manifest[length - 2] == b'0' {
        b'1'
    } else {
        b'0'
    };
    assert_eq!(
        result(&source, VerificationLimits::default()),
        Err(VerificationFailure::Authentication)
    );
    assert_eq!(source.payload_access.get(), 0);
}

#[test]
fn bad_manifests_do_not_open_payloads() {
    for replacement in [
        b"nessa-export-cut/2\n".as_slice(),
        b"nessa-export-cut/1\n",
        &[0xff],
    ] {
        let mut source = valid();
        source.manifest = replacement.to_vec();
        assert!(matches!(
            result(&source, VerificationLimits::default()),
            Err(VerificationFailure::MalformedManifest | VerificationFailure::UnsupportedFormat)
        ));
        assert_eq!(source.payload_access.get(), 0);
    }
}

#[test]
fn manifest_reads_are_bounded() {
    let source = valid();
    let limits = VerificationLimits {
        manifest_bytes: 64,
        ..VerificationLimits::default()
    };
    assert_eq!(
        result(&source, limits),
        Err(VerificationFailure::ResourceLimit)
    );
    assert_eq!(source.manifest_read.get(), 65);
    assert_eq!(source.payload_access.get(), 0);
    let source = valid();
    let limits = VerificationLimits {
        manifest_bytes: source.manifest.len(),
        ..VerificationLimits::default()
    };
    assert_eq!(result(&source, limits), Ok(()));
}

#[test]
fn manifest_entry_limit_is_checked() {
    let source = valid();
    assert_eq!(
        result(
            &source,
            VerificationLimits {
                named_entries: 2,
                ..VerificationLimits::default()
            }
        ),
        Err(VerificationFailure::ResourceLimit)
    );
    assert_eq!(source.payload_access.get(), 0);
    assert_eq!(
        result(
            &source,
            VerificationLimits {
                named_entries: 3,
                ..VerificationLimits::default()
            }
        ),
        Ok(())
    );
}

#[test]
fn layout_matches_only_public_records() {
    let mut source = valid();
    source
        .bodies
        .remove(&(StoreKind::CanonicalRecords, "first".to_owned()));
    assert_eq!(
        result(&source, VerificationLimits::default()),
        Err(VerificationFailure::MissingRecord)
    );
    let mut source = valid();
    source.bodies.insert(
        (StoreKind::DeviceCredentials, "private".to_owned()),
        b"synthetic private bytes".to_vec(),
    );
    assert_eq!(
        result(&source, VerificationLimits::default()),
        Err(VerificationFailure::UnexpectedRecord)
    );
}

#[test]
fn corrupted_and_truncated_bodies_are_refused() {
    for bytes in [b"ab".as_slice(), b"abx", b"abcd"] {
        let mut source = valid();
        source.bodies.insert(
            (StoreKind::CanonicalRecords, "first".to_owned()),
            bytes.to_vec(),
        );
        assert_eq!(
            result(&source, VerificationLimits::default()),
            Err(VerificationFailure::BodyFingerprint)
        );
    }
}

#[test]
fn body_reads_share_one_budget() {
    let source = valid();
    assert_eq!(
        result(
            &source,
            VerificationLimits {
                total_body_bytes: 5,
                ..VerificationLimits::default()
            }
        ),
        Err(VerificationFailure::ResourceLimit)
    );
    assert_eq!(source.body_read.get(), 6);
    let mut source = valid();
    source.repeat_bodies = true;
    assert_eq!(
        result(
            &source,
            VerificationLimits {
                total_body_bytes: 10,
                ..VerificationLimits::default()
            }
        ),
        Err(VerificationFailure::ResourceLimit)
    );
    assert_eq!(source.body_read.get(), 11);
    let source = valid();
    let total = source.bodies.values().map(|body| body.len() as u64).sum();
    assert_eq!(
        result(
            &source,
            VerificationLimits {
                total_body_bytes: total,
                ..VerificationLimits::default()
            }
        ),
        Ok(())
    );
}

#[test]
fn inventory_reads_and_entries_are_bounded() {
    let source = from_cuts(
        WRITER_INVENTORY
            .iter()
            .map(|kind| {
                if *kind == StoreKind::DeletionInventory {
                    SourceCut::new(
                        *kind,
                        Watermark::new("0").unwrap(),
                        vec![public(*kind, "log", &[b'x'; 128])],
                        vec![],
                    )
                    .unwrap()
                } else {
                    SourceCut::absent(*kind).unwrap()
                }
            })
            .collect(),
    );
    assert_eq!(
        result(
            &source,
            VerificationLimits {
                inventory_bytes: 20,
                ..VerificationLimits::default()
            }
        ),
        Err(VerificationFailure::ResourceLimit)
    );
    assert_eq!(source.body_read.get(), 21);
    // Three manifest entries fit. Four inventory entries do not.
    let source = bundle("nessa-deletion-inventory/1\n1 conversation a\n2 conversation b\n3 conversation c\n4 conversation d\n", "4");
    assert_eq!(
        result(
            &source,
            VerificationLimits {
                named_entries: 3,
                ..VerificationLimits::default()
            }
        ),
        Err(VerificationFailure::ResourceLimit)
    );
}

#[test]
fn authenticated_inventory_structure_and_watermark_are_checked() {
    let source = bundle("nessa-deletion-inventory/1\n1 conversation saved\n", "1");
    let report = verify_bundle(&source, &KEY, VerificationLimits::default()).unwrap();
    assert!(matches!(
        report.inventory,
        InventoryVerification::Verified { entries: 1 }
    ));
    assert_eq!(report.public_records_verified, 3);
    let cases = [
        (
            "nessa-deletion-inventory/2\n",
            "0",
            VerificationFailure::UnsupportedInventory,
        ),
        (
            "nessa-deletion-inventory/1\n1 conversation a",
            "1",
            VerificationFailure::MalformedInventory,
        ),
        (
            "nessa-deletion-inventory/1\n2 conversation a\n",
            "1",
            VerificationFailure::MalformedInventory,
        ),
        (
            "nessa-deletion-inventory/1\n1 conversation a\n2 conversation a\n",
            "2",
            VerificationFailure::MalformedInventory,
        ),
        (
            "nessa-deletion-inventory/1\n1 conversation %61\n",
            "1",
            VerificationFailure::MalformedInventory,
        ),
        (
            "nessa-deletion-inventory/1\n",
            "9",
            VerificationFailure::InventoryWatermark,
        ),
    ];
    for (text, watermark, expected) in cases {
        let source = bundle(text, watermark);
        assert_eq!(
            result(&source, VerificationLimits::default()),
            Err(expected),
            "{text:?}"
        );
    }
}

#[test]
fn inventory_requires_its_single_log_record() {
    for names in [vec!["other"], vec!["log", "extra"]] {
        let source = from_cuts(
            WRITER_INVENTORY
                .iter()
                .map(|kind| {
                    if *kind == StoreKind::DeletionInventory {
                        SourceCut::new(
                            *kind,
                            Watermark::new("0").unwrap(),
                            names
                                .iter()
                                .map(|name| public(*kind, name, b"nessa-deletion-inventory/1\n"))
                                .collect(),
                            vec![],
                        )
                        .unwrap()
                    } else {
                        SourceCut::absent(*kind).unwrap()
                    }
                })
                .collect(),
        );
        assert_eq!(
            result(&source, VerificationLimits::default()),
            Err(VerificationFailure::MalformedInventory)
        );
    }
}

#[test]
fn an_authenticated_partial_cut_is_reported_honestly() {
    let source = from_cuts(
        WRITER_INVENTORY
            .iter()
            .map(|kind| match kind {
                StoreKind::CanonicalRecords => SourceCut::with_deferred(
                    *kind,
                    Watermark::new("cut").unwrap(),
                    vec![],
                    vec![],
                    vec![DeferredEntry::new(
                        *kind,
                        "large-session",
                        65 * 1024 * 1024,
                        Fingerprint::of(b"deferred state"),
                    )
                    .unwrap()],
                )
                .unwrap(),
                StoreKind::DeviceCredentials => SourceCut::new(
                    *kind,
                    Watermark::new("cut").unwrap(),
                    vec![CapturedRecord::new(
                        *kind,
                        "private",
                        Sensitivity::Secret,
                        b"synthetic secret".to_vec(),
                    )
                    .unwrap()],
                    vec![],
                )
                .unwrap(),
                StoreKind::DeletionInventory => SourceCut::new(
                    *kind,
                    Watermark::new("unavailable").unwrap(),
                    vec![],
                    vec![SkippedRecord::new(*kind, "store", SkipReason::Unavailable).unwrap()],
                )
                .unwrap(),
                _ => SourceCut::absent(*kind).unwrap(),
            })
            .collect(),
    );
    let report = verify_bundle(&source, &KEY, VerificationLimits::default()).unwrap();
    assert_eq!(report.public_records_verified, 0);
    assert!(matches!(
        report.inventory,
        InventoryVerification::NotIncluded
    ));
    assert_eq!(report.omissions.len(), 3);
    assert_eq!(report.inventory_freshness, "not_assessed");
    assert_eq!(report.restore_readiness, "not_assessed");
    assert_eq!(report.payload_compatibility, "not_assessed");
    let output = serde_json::to_string(&report).unwrap();
    assert!(!output.contains("synthetic secret"));
}
