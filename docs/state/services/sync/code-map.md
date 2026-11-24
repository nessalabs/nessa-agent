---
id: sync-code-map
title: Where the sync code lives
kind: guide
status: mixed
summary: File paths and functions for the sync library, the gateway source, and each follower.
parent: sync
sourceRevision: "f2d721112a96beeb4b335314d56a90083acde9e9"
sources:
  - Cargo.toml
  - crates/nessa-client-core/src/read_only_sync/mod.rs
  - crates/nessa-client-core/src/read_only_sync/application/driver.rs
  - crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs
  - crates/nessa-conversation/src/infrastructure/catalogue_source.rs
  - crates/nessa-gateway/src/product/record_read/dispatch.rs
  - crates/nessa-peer-gateways/src/infrastructure/poller.rs
diagramLinks: {}
---

# Where the sync code lives

The pass coordinator is not in this repository. Workspace `Cargo.toml` pins `nessa-sync` to git revision `f2a05ef24fcff66df9d55e508539cec718e2d805` with feature `transport`. The functions named in the first table are that crate's public API, read from the cargo git checkout of that revision. Everything after that table is a path in this repository.

A host calls the library. The library calls `ScopeAuthorizer`, `RecordSource` or `CatalogueSource`, and `ReplicaStore` or `CatalogueStore`. This repository implements those traits in the gateway adapters and the SQLite cache.

## Pinned library

| Piece | Function or type | Role |
| --- | --- | --- |
| Crate root | `nessa_sync::replication` | Records, catalogue, artifacts, and history. No background work. |
| Record pass | `begin_pass`, `step`, `finish_pass` | Authorize, freeze a head, copy one validated page. `finish_pass` has no caller here. |
| Record rules | `validate_page`, `validate_page_request`, `Scope`, `Record`, `CommitPlan` | Pure checks. `Id` is at most 128 bytes. |
| Record errors | `SyncError`, `SourceError`, `StoreError`, `Access` | Denial, identity change, stale, uncertain, conflict, fence. |
| Catalogue pass | `begin_or_resume`, `apply_next_page`, `reset_catalogue` | Finite current-value walk. |
| Catalogue bounds | `MAX_CATALOGUE_ENTRIES` (256), `MAX_CATALOGUE_PAYLOAD_BYTES` (1 MiB) | Enforced by the library and consumed by Nessa. |
| Catalogue rules | `validate_manifest`, `validate_resolved`, `validate_catalogue_revision_transition` | Echo, payload, and deletion order. |
| Artifacts | `ArtifactManifest`, `MAX_CHUNK_BYTES` (64 KiB) | Manifest types plus the chunk ceiling. Live Nessa code uses the ceiling. |
| History | `TailRequest`, `OlderRequest`, `HistoryProgress` | Tail and backfill. No caller in this repository. |

## Gateway, the origin

| Path | Symbol | Role |
| --- | --- | --- |
| `crates/nessa-sdk/src/infrastructure/session_storage/record_writer.rs` | `RecordWriter` | Semantic save owner. Units stay private until a durable completion. |
| `crates/nessa-sdk/src/infrastructure/session_storage/record.rs` | `append_encoded_batch` | Physical append of encoded records. |
| `crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs` | `NessaRecordSource::head`, `page` | `RecordSource` over committed frames. Worker commands `Head` and `Page`. |
| `crates/nessa-sdk/src/infrastructure/session_storage/record_changes.rs` | `RecordChanges` | Wakes in-process subscribers after a commit. |
| `crates/nessa-sdk/src/infrastructure/session_storage/transcript.rs` | `TranscriptFold::apply`, `downloaded`, `applied` | Semantic fold and the two cursors. |
| `crates/nessa-conversation/src/infrastructure/catalogue_source.rs` | `NessaCatalogueSource` | `CatalogueSource` over conversation metadata. |
| `crates/nessa-conversation/src/application/record_read/read.rs` | record read use case | Admission and the tracked physical read for a receiver. |
| `crates/nessa-gateway/src/product/record_read/dispatch.rs` | product dispatch | `conversation.recordsHead` and `conversation.recordsPage`. |
| `crates/nessa-gateway/src/product/catalogue_read/dispatch.rs` | product dispatch | Catalogue head, manifest, and resolve. |
| `crates/nessa-protocol/src/product/generated.rs` | `CONVERSATION_RECORDS_HEAD`, `CONVERSATION_RECORDS_PAGE`, `CONVERSATION_CATALOGUE_*` | Method name constants. |
| `crates/nessa-protocol/src/conversation/read_scope.rs` | `validate_record_selector`, `validate_catalogue_selector` | Owner, receiver, and epoch checks. |
| `crates/nessa-protocol/src/product/record_read.rs` | page encode | Gateway-side `validate_page` before a response is written. |

## Followers

| Path | Symbol | Role |
| --- | --- | --- |
| `crates/nessa-client-core/src/read_only_sync/mod.rs` | module docs | Example client and the core the retained peer reader reuses. |
| `crates/nessa-client-core/src/read_only_sync/application/driver.rs` | `run_records`, `run_catalogue` | Finite loops over `begin_pass` / `step` and `begin_or_resume` / `apply_next_page`. |
| `crates/nessa-client-core/src/read_only_sync/application/follow.rs` | `follow`, `discover`, `PREPARING_ATTEMPTS` | Subscribe, recheck, and copy when the cursor changes. |
| `crates/nessa-client-core/src/read_only_sync/entrypoint/arguments.rs` | `HELP` | `sync-records`, `check-records`, `follow`, `sync-catalogue`, `reset-records`, `reset-catalogue`. |
| `crates/nessa-client-core/src/read_only_sync/infrastructure/gateway/sources.rs` | `GatewayAuthorizer`, `RecordGatewaySource`, `CatalogueGatewaySource` | Port methods become product RPCs. |
| `crates/nessa-client-core/src/read_only_sync/infrastructure/gateway/session.rs` | `GatewayConnection::run`, `keeps_connection` | One deadline per operation. Preparing and wrong-owner keep the socket. |
| `crates/nessa-client-core/src/read_only_sync/infrastructure/cache/records.rs` | `ReadOnlyCache::apply_plan` | Atomic record insert, fold, and progress. |
| `crates/nessa-client-core/src/read_only_sync/infrastructure/cache/catalogue.rs` | `apply_catalogue`, `CatalogueStore::reset` | Catalogue page commit. Trait `reset` returns `ResetAttributionRequired`. |
| `crates/nessa-client-core/src/read_only_sync/domain/reset.rs` | `CacheReset` | Explicit reset for the same receiver and stream. |
| `crates/nessa-client-core/src/read_only_sync/application/link.rs` | `run_quantum` | Weak-link pacing. Tests and `measure.rs` only. |
| `crates/nessa-client-core/src/retained.rs` | `RetainedCache::read`, `follow_grants` | Device, phone, and peer orchestration of the same drivers. |
| `crates/nessa-peer-gateways/src/infrastructure/poller.rs` | peer cycle | Fresh enrollment, then `follow_grants`. Reset and damage empty the cache and read once more. |

## Related readers that are not the follower cache

| Path | Symbol | Role |
| --- | --- | --- |
| `crates/nessa-gateway/src/webhooks/infrastructure/runtime/delivery.rs` | webhook delivery cycle | Pages the canonical record read into the outbox. |
| `crates/nessa-webhooks/src/infrastructure/outbox/cursor.rs` | cursor `observe` | Same source keeps the position. A new source scope returns `ResetRequired`. |
| `crates/nessa-webhooks/src/infrastructure/outbox/queue.rs` | enqueue | Requires `expected_after + 1` to equal `position`. |
| `src/desktop/workspace/model/revision.ts` | `supersedes`, `fromSource` | Larger view revision wins. |
| `src/desktop/workspace/application/usecases/updates.ts` | workspace merge | Applies those revisions to summaries and transcripts. |
| `src/desktop/workspace/application/ports.ts` | `{ kind: "resync" }` | Reread the workspace index. |
| `crates/nessa-attachments/src/infrastructure/store/source.rs` | range reads | Local bytes, capped by `MAX_CHUNK_BYTES`. |
| `crates/nessa-attachments/src/infrastructure/store.rs` | module comment | Protected transport remains a separate integration. |

## Open questions

- `crates/nessa-server` composition wires the gateway readers and the peer poller. The exact constructor that mounts `RetainedCache` for a peer was traced through `poller.rs` and `retained.rs`, and the server root's full dependency list was not re-walked for this map.
- Tests under `crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs` drive `begin_pass` against a loopback. That probe is test-only. It is not a second production follower.

## Further reading

[Workspace pin](../../../../Cargo.toml) · [Read-only module](../../../../crates/nessa-client-core/src/read_only_sync/mod.rs) · [SDK record source](../../../../crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs)
