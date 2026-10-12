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

## Enrollment lifecycle ordering (next implementation slice)

The design review requires a long-lived native owner to reuse the existing
physical enrollment worker and purge policy. The private-store substitution
contract retains one store/cache scope per admitted operation, an immutable
issued binding for an intent, and atomic compare-and-remove. Concurrent
out-of-band replacement is not promised; a stronger promise would require a
storage-owned pre-effect reservation. Scope replacement drains the old owner.

| Ordering/input | Owner/result | Required evidence |
| --- | --- | --- |
| Unpaired owner enrolls, later ends credential A, then enrolls B | Purge policy derives each receiver from its exact current issued credential | One wrapper, changing issued bindings, receiver assertions |
| Terminal operation expects A, saved credential is B | Existing purge policy checks intent before opening cache; typed Conflict | Zero cache opens, no credential removal, no B row deletion |
| Pending-only Unclaimed | Existing pending removal, no receiver purge | Cache opener never called |
| Purge fails | Preserve issued credential and typed purge failure | Storage/open failure and retry |
| Purge succeeds, removal fails or is uncertain | Keep original purge receipt separate from removal acknowledgement | Reopen each store; later NoPending does not erase durable purge evidence |
| Purge commits, then private-state worker panics before completion capture | Typed worker fault, no captured completion claim; durable receipt recoverable independently | Subsequent NoPending cannot inherit the abandoned transient purge effect |
| Refused/offline/Active after an earlier purge failure | Collect this operation's evidence inside its physical permit | Prior effect cannot leak into the next result |
| Caller cancels; physical worker continues | Keep existing admission until real completion; next caller remains Busy | Held worker, cancel, second caller, drain |
| Shutdown waiter cancels or last caller drops | Existing close/wake/drain lifecycle remains the physical owner | Actual closure drain, no new admission |
| Retry confirms original Unclaimed, then handshake or publication fails | Preserve recovery state; no purge authority from retry error | Both failure phases; current retry retains original receipt only on total success |

An operation's transient purge effect must be captured before its physical
permit is released, including begin/finish collection. A Busy call cannot reset
another worker's evidence. Durable historical receipts are read independently
of another successful status RPC. Public results remain immutable. These rows
are implementation requirements, not current phone guarantees.

## Alternatives considered

- Swift checkpoint/fold implementation: duplicates the shared core's authority, validation and semantic rules.
- Calling the CLI and parsing its output: unavailable on iOS and exposes presentation as a library contract.
- Treating saved product views as retained sync: cannot establish physical progress, tombstones or native paired authority.

## Consequences

The phone and existing clients share the actual fold/checkpoint rules. Public APIs, explicit reader selection and native binary distribution require tests and platform maintenance. First-slice library exposure is not phone integration or completion of the wider sync goal; native enrollment, retained projection, revocation, artifact recovery and weak-link/device experiments remain explicit gates.

## Native bridge design review

The next slice uses generated C/Swift declarations (UniFFI synchronous exports),
immutable foreign snapshot objects and a dedicated serial Swift queue with
admission before enqueueing. It retains one private state/cache scope and one
runtime; no process-global runtime, second fold or CLI parser. The existing
`PinnedStatus` classification is promoted from CLI composition into its original
application device owner for both consumers. Consent correlation remains in
Auth/protocol. Retained disclosure is per authenticated intent; reopening has no
invented disclosure.

| Additional native ordering | Required result / verification |
| --- | --- |
| Second Swift call before first queued dispatch | Reject Busy before enqueue; direct generated callers also receive Rust Busy |
| Stop before retained read registration | Retain stop state through registration; existing bounded read wakes |
| Close before dispatch or while physical read runs | Close admission first, stop/drain actual owner before scope replacement |
| Repeated close / final foreign handle drop | No reopened admission or premature private lock release |
| Swift waiter cancels before dispatch or while admitted | Stop latch is reserved before enqueue; admission remains occupied until the queued physical call settles |
| Swift close waiter cancels, another close joins | One retained drain task continues independently of waiters; new work is refused and replacement waits for actual drain |
| Last Swift worker reference drops without close | Signal native close and retain the native handle in a queued off-UI drain; no blocking native deallocation on the UI caller |
| Status, binding load, read, then refusal recheck | One driver admission; preserve separate original status/purge, read/withdrawals and recheck facts |
| Active status, confirmed record page, receiver epoch changes, read Refused, pinned recheck Refused | Keep confirmed partial saved progress; neither generic refusal authorizes purge. Native Rust and generated Swift epoch regressions assert both original and recheck payloads, then reopen saved history offline |
| Unknown status delivery after purge | Recover historical durable receipt separately; no claim of absent purge |
| Offline saved page versus later page | Compare actual full scope/progress/generation; do not combine another snapshot silently |
| Native buffer/handle lifetime and large progress | Generated ABI linked/executed under Swift 6; owned object/buffer release and UInt64 precision |
| View serialization exceeds published bound | Explicit serialization failure, no partial view or fabricated NotLoaded |

Astra medium reviewed the design and identified these ordering requirements before
implementation. Actual core linking, Swift queue admission/cancellation, paired
reads, device/simulator XCFramework execution and phone repository integration are
not yet established by that review.

## Bridge implementation evidence (phone wiring remains proposed)

The generated bridge now owns one native private scope/runtime and delegates
status, purge, read selection and physical cache progress to the existing owners.
The Swift dispatch adapter reserves admission before enqueue, keeps a stop per
operation and shares a physical drain task independently of canceled waiters.
Real paired-gateway execution through Swift covers records/progress, offline
reopen, unchanged restart and authenticated revocation/purge receipt recovery.
Release libraries link for arm64 device/simulator; the disposable simulator app
executes session open, typed refusal, off-UI dispatch and close/reopen. The macOS
static package executes the paired fixture. Physical phone execution, Nessa's
repository/authority wiring, remote artifacts and durable deletion are still open.

CI includes the bridge package. Its multi-output library target exposed a test
runner omission; the runner now recognizes the actual native unit-test executable.
A reverted target-kind fix fails its regression. The shared gateway suite also
exposed an existing callback fixture race: channel closure preceded the owning
task's listener teardown. That test now consumes its existing task-completion
receipt instead of treating an OS port observation as listener identity. This
changes verification only, not callback production behavior.
