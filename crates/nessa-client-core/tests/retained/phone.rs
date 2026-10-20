//! Publication generation and catalogue presence.
use super::*;
use crate::read_only_sync::application::device::CachePurges;
use crate::retained::RetainedCache;
use nessa_auth::application::ports::Clock as WallClock;
use nessa_auth::domain::ResourceId;
use nessa_protocol::clock::Clock as MonotonicClock;
use nessa_sync::replication::catalogue::{
    CataloguePagePlan, CataloguePass, CatalogueStore, EntryKey, ManifestEntry, ManifestPage,
    ManifestRequest, ResolvedEntry,
};
use nessa_sync::replication::domain::{Id, Scope};
use serde_json::json;
use std::sync::Arc;

#[test]
fn binding_the_same_receiver_keeps_the_stamp_and_replacement_refuses_it() {
    let mut lease = PhoneLease::new();
    assert_eq!(lease.stamp(), Err(PhoneFence::Unbound));
    assert_eq!(
        lease.accept(&PhoneStamp::issue(1, "a")),
        Err(PhoneFence::Unbound)
    );
    let first = lease.bind("a").unwrap();
    assert_eq!(lease.bind("a").unwrap(), first);
    assert_eq!(lease.accept(&first), Ok(()));
    let second = lease.bind("b").unwrap();
    assert_ne!(second.generation(), first.generation());
    assert_eq!(lease.accept(&first), Err(PhoneFence::Replaced));
    assert_eq!(lease.accept(&second), Ok(()));
}

#[test]
fn a_purge_releases_the_receiver_and_a_later_bind_is_admitted() {
    let mut lease = PhoneLease::new();
    let first = lease.bind("a").unwrap();
    lease.release_receiver();
    assert_eq!(lease.accept(&first), Err(PhoneFence::Unbound));
    lease.release_receiver();
    let second = lease.bind("b").unwrap();
    assert_eq!(lease.accept(&second), Ok(()));
    assert_eq!(lease.accept(&first), Err(PhoneFence::Replaced));
}

#[test]
fn close_refuses_the_stamp_and_a_later_bind() {
    let mut lease = PhoneLease::new();
    let stamp = lease.bind("a").unwrap();
    lease.close();
    lease.close();
    assert_eq!(lease.accept(&stamp), Err(PhoneFence::Closed));
    assert_eq!(lease.bind("b"), Err(PhoneFence::Closed));
    assert_eq!(lease.stamp(), Err(PhoneFence::Closed));
}

struct Wall;
impl WallClock for Wall {
    fn unix_milliseconds(&self) -> u64 {
        5_000
    }
}
struct Time;
impl MonotonicClock for Time {
    fn elapsed_ms(&self) -> u64 {
        0
    }
}

const A: &str = "00000000-0000-0000-0000-00000000000a";
const B: &str = "00000000-0000-0000-0000-00000000000b";

fn catalogue_scope() -> Scope {
    Scope::new(
        Id::new("receiver").unwrap(),
        Id::new("origin").unwrap(),
        Id::new("catalogue").unwrap(),
        Id::new("catalogue-incarnation").unwrap(),
        Id::new("catalogue-schema").unwrap(),
        Id::new("epoch").unwrap(),
    )
}
fn entry(id: &str, revision: u64, deleted: bool) -> ResolvedEntry {
    ResolvedEntry {
        manifest: ManifestEntry {
            key: EntryKey {
                creation: 1,
                id: Id::new(id).unwrap(),
            },
            revision,
            deleted,
        },
        payload: if deleted {
            Vec::new()
        } else {
            serde_json::to_vec(&json!({
                "id": id,
                "createdAtMs": 20,
                "agent": null,
                "model": "model",
                "approvalMode": "ask",
                "summary": null
            }))
            .unwrap()
        },
    }
}
fn page(pass: CataloguePass, values: Vec<ResolvedEntry>) -> CataloguePagePlan {
    CataloguePagePlan::new(
        ManifestPage {
            request: ManifestRequest {
                pass,
                max_entries: 10,
            },
            entries: values.iter().map(|value| value.manifest.clone()).collect(),
            has_more: false,
        },
        values,
        vec![],
    )
    .unwrap()
}
fn begin(cache: &mut RetainedCache, boundary: u64) -> CataloguePass {
    let scope = catalogue_scope();
    let expected = CatalogueStore::progress(&mut cache.cache, &scope).unwrap();
    CatalogueStore::begin(&mut cache.cache, &scope, expected, boundary)
        .unwrap()
        .active
        .unwrap()
}
fn open() -> (tempfile::TempDir, RetainedCache) {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("cache");
    nessa_local_storage::create_directory(&private).unwrap();
    let cache = RetainedCache::open(
        &private.join("cache.sqlite3"),
        Arc::new(Wall),
        Arc::new(Time),
    )
    .unwrap();
    (root, cache)
}

#[test]
fn catalogue_presence_names_a_fence_and_not_an_absent_row() {
    let (_root, mut cache) = open();
    assert_eq!(
        cache.catalogue_presence("receiver", A).unwrap(),
        CataloguePresence::Absent
    );
    let pass = begin(&mut cache, 2);
    CatalogueStore::apply_page(
        &mut cache.cache,
        page(pass, vec![entry(A, 2, false), entry(B, 2, false)]),
    )
    .unwrap();
    assert_eq!(
        cache.catalogue_presence("receiver", A).unwrap(),
        CataloguePresence::Live
    );
    let pass = begin(&mut cache, 4);
    CatalogueStore::apply_page(&mut cache.cache, page(pass, vec![entry(A, 4, false)])).unwrap();
    assert_eq!(
        cache.catalogue_presence("receiver", B).unwrap(),
        CataloguePresence::Live,
        "a page that omits a saved chat is not a tombstone"
    );
    cache
        .cache
        .withdraw(&catalogue_scope(), &Id::new(B).unwrap())
        .unwrap();
    assert_eq!(
        cache.catalogue_presence("receiver", B).unwrap(),
        CataloguePresence::Absent
    );
    let pass = begin(&mut cache, 5);
    CatalogueStore::apply_page(&mut cache.cache, page(pass, vec![entry(A, 5, true)])).unwrap();
    assert_eq!(
        cache.catalogue_presence("receiver", A).unwrap(),
        CataloguePresence::Tombstone
    );
    let _receipt =
        CachePurges::purge_receiver(&mut cache.cache, &ResourceId::new("receiver").unwrap())
            .unwrap();
    assert_eq!(
        cache.catalogue_presence("receiver", B).unwrap(),
        CataloguePresence::Tombstone
    );
}
