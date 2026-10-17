//! Pure application evidence uses the real manifest/reader and an injected MAC port.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use serde_json::{json, Value};

use crate::backup::application::{
    plan_offline_restore, ManifestMacVerifier, VerificationFailure, VerificationLimits,
    VerificationSource,
};
use crate::backup::domain::{
    AuthenticatedManifest, CapturedRecord, DeferredEntry, DeletionLog, Fingerprint, Manifest,
    Sensitivity, SkipReason, SkippedRecord, SourceCut, StoreKind, Watermark, WRITER_INVENTORY,
};

#[path = "planning_assertions.rs"]
mod planning_assertions;
use planning_assertions::assert_unavailable;

struct Memory {
    manifest: String,
    bodies: BTreeMap<(StoreKind, String), Vec<u8>>,
    payload_calls: Cell<usize>,
}

impl VerificationSource for Memory {
    fn manifest(&self) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        Ok(Box::new(Cursor::new(self.manifest.as_bytes())))
    }
    fn record(
        &self,
        store: StoreKind,
        identity: &str,
    ) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        self.payload_calls.set(self.payload_calls.get() + 1);
        Ok(Box::new(Cursor::new(
            self.bodies
                .get(&(store, identity.into()))
                .unwrap()
                .as_slice(),
        )))
    }
    fn visit_records(
        &self,
        visitor: &mut dyn FnMut(StoreKind, &str) -> Result<(), VerificationFailure>,
    ) -> Result<(), VerificationFailure> {
        self.payload_calls.set(self.payload_calls.get() + 1);
        for (store, identity) in self.bodies.keys() {
            visitor(*store, identity)?;
        }
        Ok(())
    }
}

// A synthetic authentication port; the actual CLI tests use the real HMAC/key reader.
struct Mac;
impl ManifestMacVerifier for Mac {
    fn verify(&self, canonical: &str, mac: Fingerprint) -> Result<(), VerificationFailure> {
        if Fingerprint::of(canonical.as_bytes()) == mac {
            Ok(())
        } else {
            Err(VerificationFailure::Authentication)
        }
    }
}

fn source(omissions: bool) -> Memory {
    let mut bodies = BTreeMap::new();
    let cuts = WRITER_INVENTORY
        .iter()
        .map(|kind| {
            let mut records = Vec::new();
            let mut skipped = Vec::new();
            let mut deferred = Vec::new();
            if *kind == StoreKind::DeletionInventory {
                let bytes = DeletionLog::default().canonical().into_bytes();
                records.push(
                    CapturedRecord::new(*kind, "log", Sensitivity::Public, bytes.clone()).unwrap(),
                );
                bodies.insert((*kind, "log".into()), bytes);
            } else if *kind == StoreKind::ConversationMetadata {
                let bytes = b"not a compatible database or provider session".to_vec();
                records.push(
                    CapturedRecord::new(*kind, "opaque", Sensitivity::Public, bytes.clone())
                        .unwrap(),
                );
                bodies.insert((*kind, "opaque".into()), bytes);
                if omissions {
                    records.push(
                        CapturedRecord::new(
                            *kind,
                            "secret",
                            Sensitivity::Secret,
                            b"SYNTHETIC-SECRET".to_vec(),
                        )
                        .unwrap(),
                    );
                }
            } else if *kind == StoreKind::CanonicalRecords && omissions {
                skipped = vec![
                    SkippedRecord::new(*kind, "label", SkipReason::Unreadable).unwrap(),
                    SkippedRecord::new(*kind, "label", SkipReason::Unnameable).unwrap(),
                ];
                deferred.push(
                    DeferredEntry::new(
                        *kind,
                        "session",
                        65 * 1024 * 1024,
                        Fingerprint::of(b"unavailable"),
                    )
                    .unwrap(),
                );
            }
            SourceCut::with_deferred(
                *kind,
                Watermark::new(if *kind == StoreKind::DeletionInventory {
                    "0"
                } else {
                    "absent"
                })
                .unwrap(),
                records,
                skipped,
                deferred,
            )
            .unwrap()
        })
        .collect();
    let manifest = Manifest::new(123, cuts).unwrap();
    let mac = Fingerprint::of(manifest.canonical().as_bytes());
    Memory {
        manifest: AuthenticatedManifest::seal(manifest, mac).encoded(),
        bodies,
        payload_calls: Cell::new(0),
    }
}

fn inspect(source: &Memory) -> Value {
    let report = plan_offline_restore(source, &Mac, VerificationLimits::default()).unwrap();
    json!({"status":"inspected","report":report})
}

#[test]
fn authenticated_inventory_and_no_omissions_do_not_establish_compatibility_or_authority() {
    let value = inspect(&source(false));
    assert_unavailable(&value);
    assert_eq!(
        value["report"]["verification"]["inventory"],
        json!({"status":"verified","entries":0})
    );
    assert_eq!(value["report"]["verification"]["omissions"], json!([]));
    assert_eq!(value["report"]["omission_summary"], json!([]));
    assert!(!value["report"]["unresolved_prerequisites"]
        .as_array()
        .unwrap()
        .contains(&json!("deferred_payload_availability")));
}

#[test]
fn repeated_omissions_remain_rows_without_inventing_recovery_origins() {
    let value = inspect(&source(true));
    assert_unavailable(&value);
    assert_eq!(
        value["report"]["verification"]["omissions"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        value["report"]["omission_summary"],
        json!([
            {"store":"canonical-records","rows":3,"redacted":0,"skipped":2,"deferred":1,"source_origins":"not_established"},
            {"store":"conversation-metadata","rows":1,"redacted":1,"skipped":0,"deferred":0,"source_origins":"not_established"}
        ])
    );
    assert!(value["report"]["unresolved_prerequisites"]
        .as_array()
        .unwrap()
        .contains(&json!("deferred_payload_availability")));
}

#[test]
fn authentication_and_work_limits_precede_any_planning_payload_access() {
    let mut source = source(false);
    source.manifest = source
        .manifest
        .replacen("captured-at 123", "captured-at 124", 1);
    assert_eq!(
        plan_offline_restore(&source, &Mac, VerificationLimits::default()).unwrap_err(),
        VerificationFailure::Authentication
    );
    assert_eq!(source.payload_calls.get(), 0);
    assert_eq!(
        plan_offline_restore(
            &source,
            &Mac,
            VerificationLimits {
                manifest_bytes: 0,
                ..VerificationLimits::default()
            }
        )
        .unwrap_err(),
        VerificationFailure::ResourceLimit
    );
    assert_eq!(source.payload_calls.get(), 0);
}

#[test]
fn an_inspection_report_cannot_reserve_bytes_for_future_import() {
    let mut source = source(false);
    assert_unavailable(&inspect(&source));
    source
        .bodies
        .get_mut(&(StoreKind::ConversationMetadata, "opaque".into()))
        .unwrap()
        .push(b'!');
    assert_eq!(
        plan_offline_restore(&source, &Mac, VerificationLimits::default()).unwrap_err(),
        VerificationFailure::BodyFingerprint
    );
}
