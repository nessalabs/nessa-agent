# #627 / PR652 independent R1 contracts review

Reviewed checkout: `/workspace/nessa-agent-bounded-persistence`.
BASE: `d55de88d9c6f5794b225da05b1055c511b9dba74`.
HEAD: `a8e2f1397220cb84750ef4450b16bcd4314045c3`.
The checkout was clean at first and last inspection. No checkout source edits,
commits, Cargo builds, or Cargo tests were performed by this reviewer. This is a
fresh formal R1, not an approval inherited from an earlier #627 round.

Decision: **changes requested**. One major implementation finding and one minor
public evidence/documentation finding remain at this exact head. No blocker or
nit findings. Root owns dispositions, fixes, new-head verification and hosted CI.

## Findings

### R1-C1 — Major: OS blocking-thread creation refusal escapes typed adapter outcomes

Exact location: `crates/nessa-local-storage/src/physical_operation.rs:123`, with
the uncontained synchronous invocation at `Admission::submit`, line100.

Reachable trigger: an admitted operation submits to an originating Tokio runtime
with no existing blocking worker, and the OS refuses creation of its first
blocking worker with `EAGAIN` (thread/process resource exhaustion). The locked
Tokio1.53.1 `runtime/blocking/pool.rs:325` explicitly panics on `NoThreads`.

Expected: infrastructure submission failure is a conservative typed
`Interrupted`, translated by the owning adapter to SQLite `Uncertain`, records
`Unavailable`, secret `Publication::Unknown` / `Deletion::Unknown`, or
`AuditFailure`. It must not claim rollback or release of queued physical work.

Actual: `self.executor.spawn_blocking` panics synchronously before `submit`
returns its observer. The catches in `Job::run` and `Job::Drop` contain operation
and captured destruction faults; the failed-join mapping exists inside the
observer that this path never returns. Consequently all eight adapter call sites
bypass their typed mapping on this shared infrastructure failure. The ordinary
call/capture, missing-runtime and stopped-runtime tests do not cover it.

Personally executed minimal reproduction, authorized by root, uses a C-only
`/tmp` LD_PRELOAD shim against the already built exact central test binary:

```
/workspace/nessa-agent/target/debug/deps/nessa_local_storage-9b91eeac6b4e752d \
  --exact physical_operation::tests::unused_admission_releases_slot \
  --test-threads=1 --nocapture
```

Baseline exit0; injected OS refusal exit101; uninjected repeat exit0. Shim logs
show `pthread_create #1 -> 0 (delegated)` for the test thread, then
`pthread_create #2 -> EAGAIN` for the initial blocking worker. The first panic is
at `physical_operation.rs:123:23`, `OS can't spawn worker thread: Resource
temporarily unavailable (os error 11)`. The full stack is Tokio pool.rs:325 →
Handle::spawn_blocking → Admission::spawn:123 → Admission::submit:100 → the test's
submission expression:17. This is not a later fixture `.expect` / `.unwrap`
panicking because an interruption was correctly returned.

Evidence: `/tmp/627-r1-spawn-refusal/manifest.json`, `refuse-thread.c`,
`refuse-thread.so`, `baseline.log`, `refused.log`, `restored.log`, and separate
observed exit files. The manifest records binary, shim, logs and current helper
hashes. The original209 feature-on gate log names this same binary; helper and
central-test source hashes match original209 and currenta8 manifests. No new
Rust compilation is claimed.

Neighboring ownership caution: Tokio `spawn_task` pushes the task into its queue
before attempting thread creation (pool.rs:406 and :430–444). A containment fix
must not release the job's slot or assert that no effect can land just because
the synchronous submission call panicked. This retained-queue relationship was
inspected in the locked dependency source; this minimal reproduction does not
measure retained permit lifetime, later execution or eventual cleanup.

Violated contract: typed infrastructure failures (gate1), covered new refusal
paths (gate5), and agreement between the shared submission mechanism and the
adapters' advertised conservative failure boundary. Add this ordering to the
canonical table before fixing it (gate15), and preserve the existing single
owner and queued cleanup semantics.

Disposition: reported promptly to root and adversarial reviewer; open at a8.

### R1-C2 — Minor: canonical current evidence pointer still leads to an empty pending manifest

Exact locations: `verification/bounded-persistence/README.md:24`–26 and
`verification/bounded-persistence/evidence/consolidated/final-gates/manifest.json:2`.

Reachable trigger: a reader follows the README's current-source handoff claim,
"Final source/restack identity and guarded checks are recorded in the guarded
final manifest at handoff." The linked tracked file still says
`pending clean restack and guarded final checks`, with `gates: []`; it supplies no
tested HEAD or completed final commands.

Expected: the canonical current public pointer identifies the actual final
source and completed evidence, distinguishing original209 suites from currenta8
gates and the missing local scripts process-exit observation.

Actual: the completed authoritative current gate manifest is
`/tmp/627-corrected-final-gates/manifest.json`, now included in root's
`/tmp/627-final-handoff`, while the source-linked public handoff pointer remains
the empty placeholder. This does not negate the actual tests; it contradicts the
canonical documentation's location/completion claim.

Disposition: root agrees this is a valid public pointer/documentation correction
and plans to publish the actual gate evidence and correct the pointer/placeholder
after R1 conclusions. Not fixed in the reviewed a8 checkout. Gate10 / agreement
and evidence closure apply; do not relabel older dirty-source proofs as clean
current-head results.

## Review coverage and agreement across layers

Read the full brief, AGENTS.md and CODING_STANDARDS.md; consulted architecture,
typed DI, the SDK/local-storage/server owning module maps and changed repository
map, current API docs, canonical26-row table and SDK/MCP/storage atlas links.
Reviewed the cumulative base/head production/configuration/test/docs diffs,
current shared helper, complete central and actual adapter tests and shared
fixture, production-library benchmark harnesses, and the evidence inventory,
current versus historical provenance, relevant patches/logs/measurement tables.
Archived source checkpoints and historical copies are evidence, not active
compiled owners; they are not counted as duplicate production policy or current
test execution. Not every line of every archived full Cargo.lock or unrelated
historical test log was separately reviewed as active implementation.

The following relationships were checked together rather than accepting
individually valid types as a complete lifecycle proof:

| Relationship / review dimension | Enforcer and checked evidence |
| --- | --- |
| Authority and identity | Worker has private one-slot semaphore and no Clone/global/path authority. Admission consumes its own captured Handle and permit; there is no WorkerB/runAdmissionA API. Each adapter owns its independent Worker and private Arc state. SDK coordinator revision/publication fences and OAuth reply/generation/settlement authority remain unchanged in their application/domain owners. Independent-instance actual adapter and central tests distinguish per-instance exclusion from global exclusion. |
| Domain invariants | The diff moves synchronous effects into private state methods without adding graph or OAuth policy decisions. SQLite DTO conversion is unchanged; actual graph restoration remains `OwnershipGraph::restore` in coordinator::resume_from_store, not a newly invented adapter graph validator. Secret Publication/Deletion variants remain inputs to OAuth domain commands; shared Interrupted is infrastructure only. The benchmark creates legal roots/history and directly asserts domain restore and Open/Closed fixture state. |
| Admission and concurrency | In all eight methods, `.admit().await` precedes actual large input clone/capture and consuming synchronous `.submit`. Missing runtime is refused before effects. Handle capture precedes the semaphore wait, so a queued Send future resumes on a plain thread through its origin. Acquired-but-unused Admission and an unpolled result observer are separate tested states. Job owns the capture and permit through queued/executing cleanup; dropping JoinHandle observers detaches rather than aborts physical work. R1-C1 identifies the uncovered synchronous submission refusal. |
| Failure and cleanup | Job's operation Option is taken before automatic permit-field destruction; call catch, capture Drop catch, never-started Drop and Ready output disposal are distinct. Faulting panic payloads are forgotten without inspection. Ready plus capture cleanup failure becomes interruption and disposes output under the slot. Actual stopped-origin rejection is kept distinct from central private queued abort and detached witnesses. Last returned-output Drop, physical-frame double faults and external panic hooks remain excluded. |
| Observations and restoration | Ordinary SQLite query fault is Rejected, mutex poison is Uncertain, and only QueryReturnedNoRows produces empty. FileRecords NotFound retains its existing absence meaning; read/decode/tamper/interruption never invents None at the adapter. SQLite corrupt/illegal bodies and foreign schema remain unchanged on refusal; round-trip/reopen assertions remain. Downstream restoration policy is unchanged: OAuth owner's existing secret availability probe uses its own `.ok().flatten()` policy, which this PR does not relocate into the adapter or claim to change. No new restoration or mutable dispatch authority is introduced. |
| Audit | AuthAuditRecord identity/action/intent/generation/resource/phase clone travels unchanged to actual append. Actual post-effect faults and poison remain AuditFailure; physical bytes can exist while acknowledgement fails. Audit caller-loss ordering excludes next append until prior append/sync/captured cleanup finishes. Two writes and partial-line possibility are preserved and documented. Broader attribution, audit deadlines and resource cleanup decisions remain existing application owners. |
| Representation boundaries | SQLite DTO body/schema and FileRecords JSON/sealing/key/randomness/write/delete algorithms are preserved. Encoding/query/decode/sealing and native I/O execute within the admitted state closure, rather than introducing another application port or domain policy. Existing corruption/tamper/private-file round trips remain. No added transport DTO, settings schema or disk migration. Constructor opening remains explicitly synchronous. |
| Resource bounds | The physical bound is one owned queued/running/cleaning job per instance, not bounded total callers or retained history bytes. Input copies are made only after physical admission; each waiting future still borrows its caller's data. Job/capture ownership survives result waiter loss. One-slot serialization includes clone/encoding/validation/physical work/captured cleanup and its cost is disclosed. Cross-instance/path coordination, aggregate input byte limits and shutdown drain are not promised. |
| Public surface and organization | The default-off optional local-storage capability owns one implementation. SDK/server opt in; Worker/Admission/Interrupted are not reexported as SDK product concepts. No capacity tuning, retries, production test switches, public JoinHandle or supervisor. Test-only source inclusion owns OS gates/capture observations, no copied admission/catch/join mapping. New tests and manual benchmark harnesses stay with the owning feature/layer. README/module map/table/atlas agree with source apart from R1-C2. |

Concrete neighboring interleavings inspected: waiting B canceled while A holds;
A observer canceled queued versus running versus capture-cleaning; A physical
effect landed before a post-effect call fault; Ready effect plus capture fault
with caller present and detached; queued abort plus capture/payload Drop fault;
stopped origin before submission versus already queued work surviving shutdown;
same-instance delete versus replacement and independent-instance progress;
older coordinator snapshot arriving after newer acknowledgement. Shared Worker
does not replace coordinator::commit_snapshot's write_order/revision fence.

## Actual execution versus inspected evidence

Personally executed in this R1:

- Read-only source/status checks at exacta8; recomputed all30 entries in current
  source_hashes manifest with no mismatch (including null removed old helper
  paths).
- Verified all nine formatted archival artifacts against current hashes,
  raw-gzip hashes and decompressed exact original-byte hashes: no mismatch.
- Executed tracked read-only doc-validator: exit0,118 atlas pages and canonical
  source/link/test-name/default-off syntax checks. This is not a browser/Mermaid
  rendered acceptance.
- Independently decompressed all nine current CSV archives and checked each
  phase's21000 SQLite rows/21N1000 scenarios,1750 wall rows/7N250 scenarios and
  1000 OAuth rows/4N250 lanes.
- Exact existing central test baseline0 / narrow injected OS refusal101 /
  uninjected repeat0, with source/shim/binary/log provenance above.

Inspected rather than personally executed:

- `/tmp/627-corrected-final-gates/manifest.json`: actuala8 source identity and
  seven observed exit0 gates: format, SDK ownership21, combined six-package
  all-target Clippy, SDK docs, architecture tests, architecture check,
  frontendformat. Focused current SDK success is not represented as a fresh
  current full SDK run.
- `/tmp/627-consolidated-final-gates/manifest.json` and original209 logs:
  isolated full SDK984unit/401application/212domain/65infrastructure/38doctests,
  ignored counts13/4/0/3 respectively; actual combined six-package suite. I
  independently matched all14 exact central test names to actual PASS lines in
  that combined log, rather than trusting feature unification alone.
- Original209 → a8 removes only a cfg(test) held-admission `.ok()` before
  `.expect` in source; production, mechanism and benchmarks match. Nine artifact
  formatting changes retain exact raw original-byte provenance. Prior formatting
  and Clippy non-passes remain separate historical evidence.
- Current local scripts summary348tests/346pass/0fail/2skip has no retained final
  local process exit; it is not a local completed-command PASS. Inspected hosted
  frontend run37670126367/job112959792534 log: merge checkout includesa8 into
  exact selected base, pnpmfrontend:check continued through scripts, subsequent
  checks and successful build/cleanup. Root's hosted SUCCESS supplies the
  completed-command evidence, not an inferred local exit.
- Current26 single-site mutation records, exact patches and relevant compiled
  RED/restored GREEN evidence. Auditprobe22's original exit101 is explicitly
  reconstructed from retained actual runtime assertion; restoration hashes and
  green were actually observed on resumption. Failed shutdown assumption and
  ENOSPC compile are non-passes. Historical393 two-helper proofs do not validate
  the consolidated owner.
- Default-off NORMAL tree excludes Tokio; opt-in central14 and actual combined
  package selection include it. The unchanged workflow runs one combined
  compile, then binaries and matching doctests. No extra CI switch/job was added.

## Measurements, gates and exclusions

Complete all-scenario current/inline/restored table was inspected, including
regressing medians and tails. Restored empty write75.18µs vs inline27.91µs,
2048-open write8.881ms vs7.684ms, four scheduled per-call24.738ms vs7.851ms,
full batch wall35.338ms vs16.245ms and scripted OAuth2.466ms vs0.874ms accurately
disclose overhead. Per-call timers exclude pre-first-poll scheduling; wall timers
include spawn-to-all-complete before CSV output. Seven250-batch fixture groups
per phase are5250 total wall samples, not1000 independent samples per wall
scenario. Local debug Linux tmpfs/two runtime/eight blocking workers and scripted
zero network delay do not establish production throughput or network latency.
OS-gated heartbeat evidence establishes responsiveness independently of these
latency quantiles. No speedup is claimed.

Gates15/16: the26-row design separates admission, synchronous handoff, observer
loss, effect and cleanup; no new revision/settlement precision/retry machinery.
R1-C1 is an additional unmodeled submission ordering needing table and regression
closure. Gate17: this diff changes backend physical persistence scheduling and
ownership; no UI/layout/focus/motion, transport DTO, MCP command or product
authorization semantics are changed. Runtime responsiveness is measured at the
actual adapter. I do not claim real browser scripted verification from frontend
check, backend benchmarks or Markdown rendering. External atlas/Mermaid browser
rendering remains unverified. If root considers the broad MCP/gateway scripted
run requirement applicable to this backend slice, root must supply that separate
evidence; none was inferred here.

Remaining exclusions: hosted platformCI completion is root-owned and was pending
when the brief was supplied; this reviewer did not fetch its final verdict.
Windows/macOS execution and an actual Rust1.89 build are not personally run;
SDK/server metadata explicitly names1.89 while tested toolchain is1.99. No
cross-instance/path/process exclusion, runtime-independent completion, shutdown
drain, atomic audit JSON/newline, general panic-hook suppression, guaranteed
returned-output destruction lifetime or physical-frame double-fault recovery.
Broader existing custom adapter/domain histories, public socket receipts,
permission/gateway controls and bulk cleanup are outside this focused change's
new implementation; no unchanged repository-wide guarantees are asserted from
the isolated adapter tests.
