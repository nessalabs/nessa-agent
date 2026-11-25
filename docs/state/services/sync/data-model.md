---
id: sync-data-model
title: Sync data model
kind: contract
status: mixed
summary: The scope, record, catalogue, and cursor values the follower and the gateway agree on.
parent: sync
sourceRevision: "f2d721112a96beeb4b335314d56a90083acde9e9"
sources:
  - Cargo.toml
  - crates/nessa-protocol/src/conversation/read_scope.rs
  - crates/nessa-client-core/src/read_only_sync/infrastructure/cache/records.rs
  - crates/nessa-client-core/src/read_only_sync/infrastructure/cache/catalogue.rs
  - crates/nessa-sdk/src/infrastructure/session_storage/transcript.rs
  - crates/nessa-webhooks/src/infrastructure/outbox/cursor.rs
  - crates/nessa-attachments/src/infrastructure/store/source.rs
  - src/desktop/workspace/model/revision.ts
diagramLinks: {}
---

# Sync data model

A sync value names one receiver, one origin, and one stream, plus the incarnation, schema, and access epoch that make that stream currently readable. Records are an append-only sequence inside that exact scope. Catalogue entries are the latest value of each key inside a catalogue scope. The two stores do not share a cursor.

The types below are defined in pinned `nessa-sync` `341c9b844257403bb297f27d7ed9aecbe93e4406` unless the paragraph names a file in this repository. This page records the fields those types expose and the fields this repository stores beside them.

## Scope

`Scope` holds six opaque `Id` values: `receiver`, `origin`, `stream`, `incarnation`, `schema`, and `access_epoch`. An `Id` is at most 128 UTF-8 bytes, must contain something other than whitespace, and keeps surrounding spaces when the rest is nonblank. The constructor does not trim or case-fold.

`same_receiver_stream` compares receiver, origin, and stream. Incarnation, schema, and access epoch are outside that comparison so an explicit reset can replace them. Equality of a whole `Scope` is what authorization requires: `Access::Allowed` must echo the requested scope.

On the gateway, record scope is built by `NessaRecordSource::scope` from the source identity plus the receiver and access epoch the host already checked. Catalogue scope is checked by `validate_catalogue_selector` in `crates/nessa-protocol/src/conversation/read_scope.rs` before a source read. A saved follower row whose scope differs from the requested scope is a mismatch, not a silent rewrite.

## Records and checkpoints

A `Record` is `position`, `id`, `scope`, and an opaque `payload`. Positions start at 1. A `Checkpoint` is a scope plus the last position saved with every preceding record. Position 0 means the replica is empty.

A `PageRequest` asks for records with `after < position <= target`, under `max_records`, `max_payload_bytes`, and `max_record_bytes`. `Limits` rejects a zero ceiling. `validate_page` builds a `CommitPlan` with the checkpoint it expected and the checkpoint the page would reach. The store must still compare that expected checkpoint inside one atomic write and refuse a fact id that was already saved with a different meaning.

`Pass` is in memory for one invocation: the scope, a `target` captured from one head read, and `next`, the last position applied in this pass. `is_complete` is `next == target`. Source writes after that head do not move `target`.

## What the follower stores for records

`ReadOnlyCache` keeps a `CachedProgress` row with `scope`, `downloaded`, `applied`, `facts`, and `generation`, plus the raw record rows and a `TranscriptFold` checkpoint. `downloaded` follows `TranscriptFold::downloaded`, the physical frame offset. `applied` follows `TranscriptFold::applied`, the last validated durable outer-save completion. `facts` counts semantic units in published saves. Completion envelopes and aborts add none.

`observe_head` records that an authenticated check saw a head. `mark_unknown` records that the check failed. Neither call rolls back a committed page.

## Catalogue progress

`CatalogueProgress` has the scope, `completed` (the committed revision bound), `active` (the in-flight `CataloguePass`, if a walk is unfinished), and `generation` (a compare-and-swap for begin and reset). `CataloguePass` carries the scope and the manifest cursor for that walk.

A `ManifestEntry` names an `EntryKey` and a revision, and says whether that revision is a deletion. A `ResolvedEntry` is the payload for a key the cache does not already hold at an equal or newer revision. `MAX_CATALOGUE_ENTRIES` is 256. `MAX_CATALOGUE_PAYLOAD_BYTES` is 1 MiB. Both constants are the library's.

`validate_catalogue_revision_transition` accepts a later revision of the same key. The same revision must keep the same deletion bit. A live payload after a deleted revision is `DeletionFence`.

The cache's `CatalogueStore::reset` does not implement that library transition. It returns `ResetAttributionRequired`. The operator path is `ReadOnlyCache::reset_catalogue`, which takes an attributed `CacheReset`.

## Other cursors in this repository

| Cursor | Owner | What the number means |
| --- | --- | --- |
| Webhook outbox cursor | `nessa-webhooks` `cursor.rs` | Last queued physical position for one registration and conversation, stored with the `Scope` it was read under |
| Desktop `Revised` | `src/desktop/workspace/model/revision.ts` | Per-object replacement count from the gateway view. Summary and conversation counters are never compared |
| Attachment range | `nessa-attachments` `store/source.rs` | Byte offset and length of a local held blob, capped by `MAX_CHUNK_BYTES` (64 KiB) |
| History progress | `nessa-sync` `replication::history` | `live_head`, `lower_bound`, and `oldest_available` for a tail plus older pages. No caller in this repository |

## Open questions

- Wire schemas in `nessa-protocol` mirror catalogue and record pages. The library remains the owner of `validate_page` and `validate_manifest`. A field that exists only on the wire and is dropped before those functions was not separately inventoried for this page.
- `generation` on record `CachedProgress` and `generation` on `CatalogueProgress` are different counters. The record field participates in the cache compare-and-swap described in [Ordering and conflict rules](ordering.md). Confirming every writer of the record generation outside `apply_plan` was not part of this pass.

## Further reading

[Transcript fold cursors](../../../../crates/nessa-sdk/src/infrastructure/session_storage/transcript.rs) · [Record cache apply](../../../../crates/nessa-client-core/src/read_only_sync/infrastructure/cache/records.rs) · [Read scope checks](../../../../crates/nessa-protocol/src/conversation/read_scope.rs)
