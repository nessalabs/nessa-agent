# 782. Native phone clients compose the shared retained receiver

## Purpose

Expose the existing device enrollment and retained record/cache owners to the native phone. The phone's derived view cache and command journal are useful but cannot stand in for physical retained checkpoints. This extends the client-core boundary from ADR 483.

- **Date:** 2026-10-12
- **Status:** proposed

## Context

`nessa-client-core` already owns pinned enrollment, bounded retained drivers, SDK folding, SQLite progress, retained tombstones and offline passive projection. The public retained reader currently selects granted-peer catalogue reads; owner-device composition and offline projection are reached through its CLI. iOS cannot run that CLI and must not copy its security/checkpoint decisions into Swift.

## Decision

Expose typed library operations over those owners, without CLI arguments/output or a second receiver. First, make catalogue reader selection explicit at the existing retained read boundary and expose bounded saved catalogue/transcript values with their actual progress and deletion state. Subsequent slices expose enrollment/status composition preserving purge-before-end ownership, a bounded C/Swift adapter and reproducible device/simulator builds, then native phone repository and artifact integration. Paths, clocks, sockets and private state remain composition dependencies; work runs off the UI thread. A live view or absent inbox row never advances a physical checkpoint or proves deletion.

| Ordering/input | Owner/result | Required evidence |
| --- | --- | --- |
| Owner versus granted read | Existing gateway source enforces each selector's scope | Actual paired owner read; existing peer tests |
| Changed epoch/incarnation or head behind checkpoint | Existing core returns reset/refusal; no fabricated progress | Saved checkpoint unchanged |
| Read interrupted after confirmed progress | Existing bounded driver keeps partial progress | Resume after process recreation |
| Offline saved list/view | Existing private cache and passive projection; no network | Real records/checkpoint plus fresh cache opener |
| Read/reset/deletion between saved page requests | Each page has its own SQLite snapshot and existing scope/progress/generation | Mixed-generation pages cannot be claimed as one snapshot |
| Deleted catalogue/record target | Existing retained fence; no synthetic resurrection | Deleted state across reopen and late record refusal |
| Saved corruption/quota | Existing cache's typed cause; no checkpoint advance | Source rule revert makes regression fail |
| Terminal pinned enrollment status (later slice) | Purge receiver with receipt before removing credential | Offline/refusal does not purge; failed purge retains enrollment |
| Old phone worker after scope replacement (later slice) | Native owner and phone lease fence publication | Delayed cancellation cannot publish another receiver |
| Swift/native buffer or task ends (later slice) | Owned adapter releases buffers/tasks; bounded resource use | Cancellation/overflow and device/simulator evidence |

## Alternatives considered

- Swift checkpoint/fold implementation: duplicates the shared core's authority, validation and semantic rules.
- Calling the CLI and parsing its output: unavailable on iOS and exposes presentation as a library contract.
- Treating saved product views as retained sync: cannot establish physical progress, tombstones or native paired authority.

## Consequences

The phone and existing clients share the actual fold/checkpoint rules. Public APIs, explicit reader selection and native binary distribution require tests and platform maintenance. First-slice library exposure is not phone integration or completion of the wider sync goal; native enrollment, retained projection, revocation, artifact recovery and weak-link/device experiments remain explicit gates.

### Saved metadata recovery classification

| Evidence | Caller outcome | Regression |
| --- | --- | --- |
| Persisted non-deleted catalogue entry has malformed metadata | CacheDamaged; saved progress is unchanged and recovery requires purge/resync | public_catalogue_refuses_corrupt_metadata_without_advancing_saved_progress |
