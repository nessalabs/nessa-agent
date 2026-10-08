# #627 consolidation plan — approved source owner, current bounded arrangement

393042a is the clean historical private-helper candidate. Its guarded format and
isolated full SDK checks passed (SDK258.21s); the subsequent combined selection
started before stop delivery and was terminated as partial. No later gate pass
is claimed. All owned runner/Cargo/node descendants exited. Cargo belongs to650;
the initial plan required no Cargo until reassignment. Cargo has since been reassigned to627; current runners record CARGO_INCREMENTAL=0 after root reclaimed only the shared incremental compiler cache.

## One physical lifetime implementation

Add `crates/nessa-local-storage/src/physical_operation.rs`, exported only behind
`physical-operation`. Cargo feature is default-off and enables optional Tokio
`default-features=false`, production features `rt,sync`. SDK/server opt in on
existing local-storage dependencies. Sync-only normal dependency graph stays
Tokio-free. No crate, registry, controller, capacity tuning, retries or drain API.

The minimum infrastructure API is:

```rust
pub struct Worker { /* private per-instance semaphore */ }
pub struct Admission { /* private owned permit + captured Handle */ }
pub struct Interrupted; // infrastructure result only; adapter maps it

impl Worker {
    pub fn new() -> Self; // runtime-free, exactly one slot; Default delegates
    pub async fn admit(&self) -> Result<Admission, Interrupted>;
}
impl Admission {
    pub fn submit<F, R>(self, operation: F)
        -> impl Future<Output = Result<R, Interrupted>> + Send
    where F: FnMut() -> R + Send + 'static, R: Send + 'static;
}
```

Admission captures `Handle::try_current` before semaphore waiting. Synchronous
`submit` constructs the existing private Job (capture+permit), submits through
that captured executor, and returns an opaque join-result future. No WorkerB/run
AdmissionA mismatch and no unpolled async-run destruction window: dropping the
returned future before its first poll detaches an already-owned physical job.
Dropping unused Admission releases its empty slot. Constructors remain runtime-
free; reliable progress requires the origin executor/physical operation to run.

Move the chosen Job call/capture/Ready/neverstarted containment implementation
once, with unchanged panic-payload forgetting and exclusions. A private spawn
method returning JoinHandle may back `submit`, solely so this module's unit tests
can abort a queued job; no public cancellation/JoinHandle/probe API. Existing
Uncertain/Unavailable/Unknown/AuditFailure conversions remain in their adapters.

Each adapter retains its own Worker field and actual Arc state containing its
connection, directory or audit mutex. After `admit().await`, clone actual input
and construct/submit its closure in the same poll. Remove both old production
blocking modules; do not export an Input wrapper or probes.

## Test arrangement without production test switches

Central mechanic tests live beside the shared source and own the one-slot,
executor capture, dropped/unpolled result, queued cancellation, call panic,
separate capture Drop, Ready downgrade and payload-forget rules. They access
private spawn/Job internals where necessary; no extra production-visible API.
Use bounded subprocesses/counters and independent OS gates as now. Tokio dev
features may add time/multithread capabilities for these tests only; they do not
add Tokio to the default-off normal dependency graph.

One private test fixture implementation lives at local-storage's test support
path and is included with `#[cfg(test)] #[path=...]` by SDK/server test modules.
It owns the existing Gate/watchdog/plain-thread helpers and observation Probe,
not semaphore/admission/catch policy. It can wrap an actual owned FnMut closure:
counts submission/entry, gates/faults before/after the real state call, and holds a
Drop gate/fault around destruction of the inner closure's real input/state.
An inner-capture field preceding a completion ticket ensures observed cleanup
counts follow real capture destruction. This replaces private worker probes and
production Input wrappers. Production closures directly own snapshot/token/
record/state. Adapters gain only cfg(test) Probe fields and a cfg(test) closure
wrapping line; this fixture contains no duplicate executor/permit/catch logic.

Keep all actual adapter heartbeat, on-disk body/order, cross-method exclusion,
input/capture Drop, actual landed-effect uncertainty, poison, decode/tamper,
no-runtime and plain-thread regressions beside their adapters. Move tests whose
assertion is purely the helper's implementation to its unit-test owner. Avoid
claiming a central dummy operation proves an untested actual I/O method.

Actual runtime evidence corrected the original shutdown fixture assumption:
already queued spawn_blocking work executes on runtime shutdown. Preserve that
failed assumption and exact initial source as history. Actual adapter tests now
first poll real admission behind a held slot, stop the captured origin executor
BEFORE submission, and resume the real port future on a plain thread. The stopped
executor refuses the synchronous submit and destroys its actual capture under
Job admission; a second live runtime remains excluded through that cleanup.
Typed Uncertain / secret Unknown, actual disk absence, capture counts and faulting
payload counters are asserted. No detached caller claim is made inside synchronous
rejected submission. Genuine queued abort and detached neverstarted Job Drop
remain central unit contracts using PRIVATE JoinHandle access only. Root approved
this correction; no production abort switch or exported probe API exists.

## CI, documents and exact evidence refresh

`cargo-test-parallel.mjs` was inspected: it performs ONE combined `cargo test
--no-run` build and then executes its binaries. Existing six-package selection
includes SDK/server, whose dependency opt-in feature unifies onto local-storage;
central feature tests therefore participate without a new CI job or switch.
Require the actual CI log to name those tests. Also explicitly check/test/Clippy
local-storage alone default-off and opt-in, and inspect its default-off normal
Cargo dependency graph for absence of Tokio. Default-off feature coverage is not
inferred from the combined opt-in graph.

Before source edits, revise the canonical table to name the central enforcer,
per-adapter typed mappings, synchronous submit and unpolled-return/unused-admission
Drop cases. Update local-storage README/module map, SDK/server module maps,
codebase structure, storage atlas sources/owning prose and minimal SDK/MCP links.
Keep database opening and retained-directory publication diagrams unchanged.

Preserve393 source hashes,26 old probes,66k old timing samples and stopped-gate
checkpoint as historical evidence. Archive it before changing canonical current
inventory/claims. Refresh current one-owner mechanic mutations, all8 exact typed
adapter sites, admission-before-clone and actual offload heartbeat probes on the
consolidated source. Do not count old two-copy probes as current-head proof.
Rerun stable1000-sample current/inline/restored workloads without changing clock
scope or fixtures; preserve overhead/regressions and first-poll exclusion.
After restoring, self-review and commit clean, rerun guarded default-off/opt-in
checks plus isolated SDK and exact normal six-package tests/Clippy/fmt/docs/
architecture on that commit. Publish evidence-only follow-up with identical
source hashes; root owns independent review/push/PR.
