# #627 consolidated source self-review checkpoint

Identity: historical clean HEAD393042a; current dirty source is precisely hashed
in source-manifest.json (29 paths relative to intended f168 base and393, including
new files and declared deletions). This is not a formal review or final handoff.

One source owner now implements physical admission, origin-executor capture,
synchronous capture/permit handoff, call/capture/neverstarted/Ready containment,
and payload forgetting: local-storage/src/physical_operation.rs. Public capability
is exactly Worker, Admission, Interrupted; no registry, configurable capacity,
retry, drain, controller, exported probes or SDK product re-export. Job is private.
Worker instances remain independently adapter-owned. Actual connection, directory
and audit mutex remain in their existing Arc states. All eight typed mappings and
sync disk algorithms stay in the adapters; application/domain revision, OAuth and
settlement authority are untouched.

Admission::submit transfers ownership synchronously before returning its opaque
result future. This removes both mismatched Worker/admission use and an unpolled
async-run capture-destruction window. Source-included cfg(test) fixture contains
only real clone/capture observations, OS gates and fault payloads: no semaphore,
executor selection, Job, join mapping or catch policy. Production Input wrappers
and both old blocking helper implementations are deleted.

Actual default-off local-storage tests and normal Cargo tree passed (no Tokio
normal dependency); explicit opt-in passed all14 central mechanic tests. Corrected
SDK real-adapter matrix passed21. Server records20 and audit7 passed on the recovered unchanged production source
after the preserved ENOSPC failure; strengthened clone assertions are now rerunning. root reclaimed only shared incremental compiler cache, and all
subsequent runners use CARGO_INCREMENTAL=0. Normal combined six-package CI test
execution with named central tests remains required and is not inferred from
explicit opt-in results.

Failed runtime-shutdown assumption is retained separately: already queued blocking
work runs on shutdown. Actual adapter stopped-executor tests instead stop the
captured executor while real admission waits, resume the actual future on a plain
thread, and verify typed capture cleanup against a second live runtime. They do
not claim actual detached caller loss inside synchronous rejected submission.
Genuine queued-abort/detached containment is centrally exercised with private
JoinHandle access. Exact initial failed fixture source hashes were recovered and
verified equal to its original run manifest, with honest reconstruction provenance.

The26-row canonical table maps to actual enforcing function paths in the adjacent
JSON. Source/link/frontmatter validation passes118 atlas pages. Root identified a blank separator before row25. It was removed after the
scoped run. Actual markdown-it14.3.1 rendering now produces one table containing
rows1–26; the retained before source rendered only1–24. Exact source hashes, row
lists and before/after HTML are in /tmp/627-consolidated-table-render-proof.json. External atlas-browser/Mermaid render
is unavailable and unclaimed. Primitive directory-publication charts are unchanged.

Remaining scoped proof work: finish strengthened matrix; perform
single-rule current shared-owner and typed adapter mutations, stable1000-sample
current/inline/restored benchmarks, complete own standards review, commit clean,
then guarded feature-off/on + isolated SDK + exact six-package final checks.
All393 probes and timings remain historical, never current consolidated proof.

Limits remain explicit: originating executor must progress; no cross-instance/path
exclusion or shutdown drain; output destructors after Job return are outside the
slot lifetime; operation-frame double faults may abort; external panic hooks are
not suppressed; audit JSON/newline remain separate potentially partial writes.
Benchmarks are debug local tmpfs, include observed overhead/regressions and exclude
pre-first-poll scheduling wait; responsiveness comes from independent OS-gated
current-thread witnesses, not favorable timing tails.

Borrowed clone sites: SDK snapshot write -> admission_precedes_owned_input_transfer;
records auth store -> same named records test; records secret store ->
canceled_delete_excludes_replacement (actual constructor count2 after initial
publication and held delete); audit record -> worker_input_drop_keeps_slot
(actual constructor count1 while first capture cleanup holds admission). All
constructor counters are inside clone_input invocation of the real clone, not
independent counters after cloning. No new worker policy exists in those seams.
