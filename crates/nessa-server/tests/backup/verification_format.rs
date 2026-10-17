//! Canonical bundle listings and the shared live inventory decoder.

use crate::backup::domain::{
    AuthenticatedManifest, CapturedRecord, CredentialKind, DeferredEntry, DeletionFence,
    DeletionLog, DeletionReadError, Fingerprint, Manifest, ManifestListing, ManifestReadError,
    Sensitivity, SkipReason, SkippedRecord, SourceCut, StoreKind, Watermark, WRITER_INVENTORY,
};

fn listing(entries: bool) -> AuthenticatedManifest {
    let mut stores: Vec<_> = WRITER_INVENTORY
        .iter()
        .map(|kind| SourceCut::absent(*kind).unwrap())
        .collect();
    if entries {
        let store = StoreKind::CanonicalRecords;
        stores[0] = SourceCut::with_deferred(
            store,
            Watermark::new("cut cursor/é").unwrap(),
            vec![
                CapturedRecord::new(
                    store,
                    "public/name é",
                    Sensitivity::Public,
                    b"public".to_vec(),
                )
                .unwrap(),
                CapturedRecord::new(
                    store,
                    "redacted",
                    Sensitivity::Secret,
                    b"synthetic only".to_vec(),
                )
                .unwrap(),
                CapturedRecord::new(store, "another-public", Sensitivity::Public, vec![]).unwrap(),
            ],
            vec![SkippedRecord::new(store, "skipped", SkipReason::Bounded).unwrap()],
            vec![DeferredEntry::new(store, "deferred", 123, Fingerprint::of(b"stream")).unwrap()],
        )
        .unwrap();
    }
    AuthenticatedManifest::seal(
        Manifest::new(123, stores).unwrap(),
        Fingerprint::of(b"synthetic mac"),
    )
}

#[test]
fn writer_omission_rows_can_repeat_and_overlap_public_names() {
    let kind = StoreKind::ConversationAudit;
    let cases = [
        (
            vec![CapturedRecord::new(
                kind,
                "secret-row",
                Sensitivity::Secret,
                b"synthetic".to_vec(),
            )
            .unwrap()],
            vec![SkippedRecord::new(kind, "secret-row", SkipReason::Secret).unwrap()],
        ),
        (
            vec![],
            vec![
                SkippedRecord::new(kind, "unnameable", SkipReason::Unnameable).unwrap(),
                SkippedRecord::new(kind, "unnameable", SkipReason::Unnameable).unwrap(),
            ],
        ),
        (
            vec![],
            vec![
                SkippedRecord::new(kind, "line-1", SkipReason::Unreadable).unwrap(),
                SkippedRecord::new(kind, "line-1", SkipReason::Unreadable).unwrap(),
            ],
        ),
        (
            vec![CapturedRecord::new(
                kind,
                "line-1",
                Sensitivity::Public,
                b"synthetic public".to_vec(),
            )
            .unwrap()],
            vec![SkippedRecord::new(kind, "line-1", SkipReason::Unreadable).unwrap()],
        ),
    ];
    for (records, skipped) in cases {
        let mut stores: Vec<_> = WRITER_INVENTORY
            .iter()
            .map(|kind| SourceCut::absent(*kind).unwrap())
            .collect();
        let index = WRITER_INVENTORY
            .iter()
            .position(|stored| *stored == kind)
            .unwrap();
        let rows = records.len() + skipped.len();
        stores[index] = SourceCut::new(
            kind,
            Watermark::new("cut").unwrap(),
            records.clone(),
            skipped.clone(),
        )
        .unwrap();
        let sealed = AuthenticatedManifest::seal(
            Manifest::new(123, stores).unwrap(),
            Fingerprint::of(b"synthetic mac"),
        );
        let raw = sealed.encoded();
        let listing = ManifestListing::parse(&raw, rows).unwrap();
        let store = &listing.stores()[index];
        assert_eq!(store.skipped(), skipped);
        assert_eq!(store.records().len(), records.len());
        for (listed, original) in store.records().iter().zip(&records) {
            assert_eq!(listed.identity(), original.identity());
            assert_eq!(listed.fingerprint(), original.fingerprint());
            assert_eq!(listed.is_public(), original.public_bytes().is_some());
        }
        assert_eq!(
            ManifestListing::parse(&raw, rows - 1),
            Err(ManifestReadError::EntryLimit)
        );
    }
    let raw = listing(true).encoded();
    let redacted = format!(
        "redacted canonical-records redacted {}\n",
        Fingerprint::of(b"synthetic only").hex()
    );
    let deferred = format!(
        "deferred canonical-records deferred 123 {}\n",
        Fingerprint::of(b"stream").hex()
    );
    for allowed in [
        raw.replacen(
            "redacted canonical-records redacted",
            "redacted canonical-records public%2Fname%20%C3%A9",
            1,
        ),
        raw.replacen(
            "deferred canonical-records deferred",
            "deferred canonical-records redacted",
            1,
        ),
        raw.replacen(
            "skipped canonical-records skipped",
            "skipped canonical-records deferred",
            1,
        ),
        raw.replacen(&redacted, &(redacted.clone() + &redacted), 1),
        raw.replacen(&deferred, &(deferred.clone() + &deferred), 1),
    ] {
        let parsed = ManifestListing::parse(&allowed, 100).unwrap();
        let first = &parsed.stores()[0];
        assert_eq!(
            first.records().len() + first.deferred().len() + first.skipped().len(),
            allowed.lines().count() - (WRITER_INVENTORY.len() + 3)
        );
    }
}

#[test]
fn canonical_listings_keep_metadata_and_the_mac_offset() {
    let sealed = listing(true);
    let raw = sealed.encoded();
    let read = ManifestListing::parse(&raw, 5).unwrap();
    assert_eq!(read.canonical_len(), sealed.manifest().canonical().len());
    assert_eq!(&raw[..read.canonical_len()], sealed.manifest().canonical());
    assert_eq!(read.mac(), *sealed.mac());
    assert_eq!(read.captured_at_ms(), 123);
    assert_eq!(read.stores().len(), 22);
    let first = &read.stores()[0];
    assert_eq!(first.store(), StoreKind::CanonicalRecords);
    assert_eq!(first.watermark().as_str(), "cut cursor/é");
    assert_eq!(first.records()[0].identity(), "public/name é");
    assert_eq!(
        first.records()[0].encoded_identity(),
        "public%2Fname%20%C3%A9"
    );
    assert_eq!(first.records()[0].fingerprint(), Fingerprint::of(b"public"));
    assert!(first.records()[0].is_public());
    assert!(!first.records()[1].is_public());
    assert!(first.records()[2].is_public());
    assert_eq!(first.deferred()[0].size(), 123);
    assert_eq!(first.skipped()[0].reason(), SkipReason::Bounded);
    assert!(ManifestListing::parse(&listing(false).encoded(), 0).is_ok());
    assert_eq!(
        ManifestListing::parse(&raw, 4),
        Err(ManifestReadError::EntryLimit)
    );
}

#[test]
fn malformed_listing_grammar_is_refused() {
    let raw = listing(true).encoded();
    let first_hash = Fingerprint::of(b"public").hex();
    let record = format!("record canonical-records public%2Fname%20%C3%A9 {first_hash}\n");
    let deferred = format!(
        "deferred canonical-records deferred 123 {}\n",
        Fingerprint::of(b"stream").hex()
    );
    let skipped = "skipped canonical-records skipped bounded\n";
    let cases = vec![
        raw.replacen("captured-at 123", "captured-at 0123", 1),
        raw.replacen("captured-at 123", "captured-at +123", 1),
        raw.replacen("captured-at 123", "captured-at 18446744073709551616", 1),
        raw.replacen("public%2Fname", "public%2fname", 1),
        raw.replacen("public%2Fname", "public/name", 1),
        raw.replacen("public%2Fname", "%70ublic%2Fname", 1),
        raw.replacen("public%2Fname", "public%GGname", 1),
        raw.replacen("public%2Fname", "public%FFname", 1),
        raw.replacen("public%2Fname", "public%00name", 1),
        raw.replacen(&first_hash, &first_hash.to_uppercase(), 1),
        raw.replacen(&first_hash, &format!("é{}", "a".repeat(62)), 1),
        raw.replacen(&record, &(record.clone() + &record), 1),
        raw.replacen(&deferred, &(deferred.clone() + &record), 1),
        raw.replacen(
            &deferred,
            &(deferred.clone()
                + &record.replace("public%2Fname%20%C3%A9", "unique-after-deferred")),
            1,
        ),
        raw.replacen(skipped, &(skipped.to_owned() + &deferred), 1),
        raw.replacen(
            skipped,
            &(skipped.to_owned() + &deferred.replace(" deferred ", " unique-after-skipped ")),
            1,
        ),
        raw.replacen("deferred 123", "deferred 0123", 1),
        raw.replacen("skipped bounded", "skipped unknown", 1),
        raw.replacen("record canonical-records", "record notes", 1),
        raw.replacen("watermark notes absent\n", "", 1),
        raw.replacen("watermark notes absent", "watermark share-grants absent", 1),
        raw.replacen("watermark notes absent", "watermark notes absent extra", 1),
        raw.replacen("public%2Fname%20%C3%A9", ".", 1),
        raw.replacen("public%2Fname%20%C3%A9", "..", 1),
        raw.replacen("public%2Fname%20%C3%A9", "", 1),
        raw.replacen("public%2Fname%20%C3%A9", &"a".repeat(513), 1),
        raw.replace('\n', "\r\n"),
        format!("{raw}\n"),
        format!("{raw}mac {}\n", Fingerprint::of(b"extra").hex()),
        raw.trim_end_matches('\n').to_owned(),
        raw[..raw.rfind("mac ").unwrap()].to_owned(),
    ];
    for (index, bad) in cases.iter().enumerate() {
        assert_eq!(
            ManifestListing::parse(bad, 100),
            Err(ManifestReadError::Malformed),
            "case {index}"
        );
    }
    assert_eq!(
        ManifestListing::parse(
            &raw.replacen("nessa-export-cut/1", "nessa-export-cut/2", 1),
            100
        ),
        Err(ManifestReadError::UnsupportedVersion)
    );
    assert_eq!(
        ManifestListing::parse("unrelated\n", 100),
        Err(ManifestReadError::Malformed)
    );
}

#[test]
fn deletion_decoder_reuses_live_grammar_without_tightening_it() {
    let (log, _) =
        DeletionLog::default().append(DeletionFence::conversation("conversation/name").unwrap());
    let (log, _) = log.append(
        DeletionFence::credential(CredentialKind::Device, "old", Fingerprint::of(b"synthetic"))
            .unwrap(),
    );
    let canonical = log.canonical();
    assert_eq!(DeletionLog::decode(&canonical).unwrap(), log);
    let noncanonical = canonical.replace("%2F", "%2f").replace(
        &Fingerprint::of(b"synthetic").hex(),
        &Fingerprint::of(b"synthetic").hex().to_uppercase(),
    ) + "\n";
    assert_eq!(DeletionLog::decode(&noncanonical).unwrap(), log);
    assert_ne!(noncanonical, log.canonical());
    assert_eq!(
        DeletionLog::decode("nessa-deletion-inventory/2\n"),
        Err(DeletionReadError::UnsupportedVersion)
    );
    assert_eq!(
        DeletionLog::decode("garbage\n"),
        Err(DeletionReadError::Malformed)
    );
    assert_eq!(
        DeletionLog::decode("nessa-deletion-inventory/1\n2 conversation gap\n"),
        Err(DeletionReadError::Malformed)
    );
    assert!(Fingerprint::from_hex(&format!("é{}", "a".repeat(62))).is_err());
}

#[test]
fn listing_name_bounds_count_decoded_bytes_before_retaining_names() {
    let raw = listing(true).encoded();
    for encoded in ["a".repeat(512), "%20".repeat(512)] {
        let valid = raw.replacen("public%2Fname%20%C3%A9", &encoded, 1);
        assert!(ManifestListing::parse(&valid, 5).is_ok());
    }
    for encoded in [
        "a".repeat(513),
        "%20".repeat(513),
        "a".repeat(4 * 1024 * 1024),
    ] {
        let bad_identity = raw.replacen("public%2Fname%20%C3%A9", &encoded, 1);
        assert_eq!(
            ManifestListing::parse(&bad_identity, 5),
            Err(ManifestReadError::Malformed)
        );
        let bad_watermark = raw.replacen("cut%20cursor%2F%C3%A9", &encoded, 1);
        assert_eq!(
            ManifestListing::parse(&bad_watermark, 5),
            Err(ManifestReadError::Malformed)
        );
    }
}
