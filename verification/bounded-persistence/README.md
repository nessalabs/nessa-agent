# Bounded physical persistence verification (#627)

The current implementation uses one optional `nessa-local-storage::physical_operation`
owner and independent adapter instances. The [canonical ordering table](../../docs/design/bounded-physical-persistence.md)
and [storage atlas](../../docs/state/services/storage/README.md#physical-adapter-work)
name its boundaries and witnesses. Historical393 private-helper proofs below do
not validate the consolidated source.

Current review evidence is under [evidence/consolidated](evidence/consolidated/provenance-notes.json).
Original commands/logs preserve their local paths; published CSV files have the
same names with `.gz`, and [uncompressed hashes](evidence/consolidated/uncompressed-csv-sha256.json)
identify their exact raw bytes. [Artifact hashes](evidence/consolidated/artifact-sha256.json)
identify the published copies. Scripts are executed review artifacts, not a
supported tooling API; replay paths require adaptation.

## Current source and scoped proof

The [v2 manifest](evidence/consolidated/v2-source-checkpoint/source-manifest.json)
identifies dirty consolidated source over393042a, before restack. Its
[26-row mapping](evidence/consolidated/v2-source-checkpoint/table-to-test-mapping.json)
links actual enforcing tests. The [benchmark checkpoint](evidence/consolidated/benchmark-source-checkpoint/source-manifest.json)
adds only batch wall observation, a declared Open/Closed fixture assertion and
conditional shutdown wording; production rules are unchanged. Pre-restack source
copies remain separately archived. Final source/restack identity and guarded
checks are recorded in the [guarded final manifest](evidence/consolidated/final-gates/manifest.json) at handoff, without relabeling
these earlier runs as clean-commit runs.

Default-off local-storage tests and NORMAL dependency tree passed (no Tokio
normal dependency); explicit opt-in passed14 central mechanic tests. Actual SDK
matrix passed21; recovered server records20 and audit7 passed. Combined six-package
execution must itself name all14 central cases; explicit opt-in execution alone
is not that CI-selection proof. SDK/server declare MSRV1.89; tested rustc1.99 does
not establish an executed minimum-compiler check.

The initial already-queued shutdown fixture assumption failed and is preserved;
already queued blocking work can still succeed. Corrected real adapters instead
exercise stopped-origin submission and typed captured cleanup. Central private
JoinHandle tests own genuine queued-abort/detached cases. They do not claim an
actual adapter loses its caller during synchronous rejected submission. A server
ENOSPC compilation is retained as non-pass; unchanged-source recovery uses
CARGO_INCREMENTAL=0 after root reclaimed only incremental compiler cache.

[26 mutation records](evidence/consolidated/mutation-probes/candidate-source-manifest.json)
retain each exact patch, compiled runtime failure and byte-restored focused green
with fresh mtimes:14 central boundaries,8 distinct typed adapter outcomes and4
real clone constructors. Grouped filters do not prove untouched duplicate-name
sites. [Audit probe22](evidence/consolidated/mutation-probes/audit-ack.result.json)
resumed interrupted orchestration: its actual Ok-vs-AuditFailure runtime assertion
is retained; original exit101 is reconstructed, not newly observed. Restoration
hashes and green were actually observed on resumption.

A separate [single shared offload-owner revert](evidence/consolidated/benchmarks/async-runtime-offload-revert.patch)
failed central1 and all8 real adapter heartbeat cases; exact restoration passed.
Reconstructed inline adapters also failed SDK2/server6 heartbeats before their
measurements; restored source passed8 again. These are OS-gated current-thread
responsiveness witnesses, not timing-quantile claims or8 extra source mutations.

The actual [markdown-it proof](evidence/consolidated/final-table-render-proof.json)
renders one table with rows1–26; original separator failure remains archived.
[118-page metadata/source/link checks](evidence/consolidated/final-doc-validation.log)
pass. External atlas browser/Mermaid rendering is unavailable and unclaimed.

## Current local measurements

[Environment and clocks](evidence/consolidated/benchmarks/measurement-environment.json),
[commands/source identities](evidence/consolidated/benchmarks/run-manifest.json),
[raw-data validation](evidence/consolidated/benchmarks/measurement-validation.json),
and [every scenario median/p95/p99](evidence/consolidated/benchmarks/all-scenario-comparison.md)
cover current, reconstructed-inline and exact-restored phases. Each has21000
SQLite per-call rows and1000 scripted OAuth flows (66000 total). Seven legal
empty/open/closed root-history fixtures have0/32/256/2048 lifetime rows and zero
spawns; domain restore and declared state are checked. The global128 retained
request cap is unchanged. Encoded body bytes agree across all phases.

Original caller timers start at port invocation/task first poll and exclude prior
scheduler wait. The additional wall timer starts before spawning each existing
four-writer batch and ends after all four complete, before CSV writes. Each of
seven fixtures has250 batches per phase (1750 per phase;5250 total). These are
N250 batch samples, not1000 independent wall samples, and four scheduled callers
are not guaranteed simultaneously ready in a poll. Gathering all four returned
rows before writing CSV is an observer-only instrumentation adjustment applied
identically in every phase; per-call clocks and storage operations are unchanged.

Restored medians illustrate the cost: empty write75.18µs versus inline27.91µs;
2048-open single write8.88ms versus7.68ms; four-caller per-call write24.74ms versus
7.85ms. Full four-writer batch wall is35.34ms versus16.24ms (N250); scripted OAuth
is2.47ms versus0.87ms. All tails and scenarios, including regressions, remain in
the linked table. One admission slot serializes input cloning, validation/encoding,
physical I/O and captured cleanup. Reconstructed inline calls can perform some
encoding across two runtime workers before their connection mutex. The measured
batch regression therefore includes that serialization tradeoff, not merely
scheduler overhead; components were not timed separately. Responsiveness and
physical ownership improve while these local caller/batch latencies increase.

These are debug Linux tmpfs/SQLite DELETE-FULL and zero-delay scripted OAuth,
with two runtime workers/eight blocking workers, not production/network estimates.
Median averages the middle pair; p95/p99 use nearest rank. Limited sample tails
and run variation are disclosed; no CI timing threshold or speedup is claimed.
Output cleanup after Job return, physical/capture-frame double-fault aborts,
external panic hooks, cross-instance/path exclusion, shutdown drain and atomic
audit JSON/newline writes remain outside the stated guarantees. Browser scripted
verification is separate and has not been claimed by these backend measurements.

## Historical393 and earlier evidence

Everything below belongs to the private-helper source at393042a or earlier dirty
source. Its mutations, timings and partial final gates are historical only.

### Historical private-helper verification (#627)

Candidate base: `f1682e8a763646c6c3e73dee73c1c47987e2cbae`. The handoff provides the clean commit SHA; final gate manifest records its actual tested commit. [Changed implementation/test/docs source hashes](evidence/candidate-source-sha256.json) are computed against this base and exclude verification artifacts. An evidence-only follow-up preserves this source inventory.

Scope: one physical job per SqliteOwnershipStore, FileRecords or FileAuthorizationAudit instance through queued time, I/O and captured input cleanup. Async waiters are not a bounded total caller count. [Design/order table](../../docs/design/bounded-physical-persistence.md), [implementer review](evidence/self-review.md), and [storage atlas](../../docs/state/services/storage/README.md#physical-adapter-work) describe the enforcers and limits.

## Source and evidence provenance

- [First RED provenance](evidence/first-red-provenance.tar.gz) and [reconstructed first-RED feature source](evidence/first-red-source.tar.gz): dirty test-only edits over the base; original five actual failures. Reconstruction is archival, not a clean source commit or fresh replay. SDK direct-binary and Cargo SDK/server runs are distinguished in the provenance.
- [Runtime RED feature source](evidence/runtime-red-source.tar.gz): actual no-runtime/plain-thread regressions. `runtime-first-red.log` is a retained compile-only fixture error and is excluded from runtime RED evidence; corrected RED logs are separate.
- [Phase-one candidate feature source](evidence/phase-one-candidate-source.tar.gz) and [phase-two candidate feature source](evidence/phase-two-candidate-source.tar.gz) retain exact helper/adapter/test files and hashes used by their respective mutations. Unchanged repository sources come from the intended base. Phase two strengthens only cfg(test) Input faults and isolated witnesses; final production-library repeat uses the restored final feature source. These archives are feature files, not a claim of complete binary or dependency capture.
- [SHA256SUMS](evidence/SHA256SUMS) covers review artifacts. Logs and scripts are historical executed artifacts, not a new supported tooling API. Original paths in replay scripts require adaptation for another checkout.

## Actual mutation results

26 single-site probes compiled, failed actual runtime assertions, then passed after byte restoration with fresh mtimes. One original queued Drop removal passed both mutant/restored tests and is excluded: the static payload/awaiting caller masked removal. The exact actual Input Drop seam was strengthened with the existing faulting payload, detached caller and counter; both copies now fail with observed destructor count1 instead of0. No production fault framework was added.

| Phase | Rule/site | Mutated / restored exit | Patch and runtime log |
| --- | --- | --- | --- |
| mutations | ambient-executor | 101 / 0 | [patch](evidence/mutations/ambient-executor.patch), [mutant](evidence/mutations/ambient-executor.log), [restored](evidence/mutations/ambient-executor-restored.log) |
| mutations | call-uncontained | 101 / 0 | [patch](evidence/mutations/call-uncontained.patch), [mutant](evidence/mutations/call-uncontained.log), [restored](evidence/mutations/call-uncontained-restored.log) |
| mutations | definite-audit-success | 101 / 0 | [patch](evidence/mutations/definite-audit-success.patch), [mutant](evidence/mutations/definite-audit-success.log), [restored](evidence/mutations/definite-audit-success-restored.log) |
| mutations | definite-secret-delete | 101 / 0 | [patch](evidence/mutations/definite-secret-delete.patch), [mutant](evidence/mutations/definite-secret-delete.log), [restored](evidence/mutations/definite-secret-delete-restored.log) |
| mutations | definite-secret-publication | 101 / 0 | [patch](evidence/mutations/definite-secret-publication.patch), [mutant](evidence/mutations/definite-secret-publication.log), [restored](evidence/mutations/definite-secret-publication-restored.log) |
| mutations | early-permit-release | 101 / 0 | [patch](evidence/mutations/early-permit-release.patch), [mutant](evidence/mutations/early-permit-release.log), [restored](evidence/mutations/early-permit-release-restored.log) |
| mutations | input-before-admission | 101 / 0 | [patch](evidence/mutations/input-before-admission.patch), [mutant](evidence/mutations/input-before-admission.log), [restored](evidence/mutations/input-before-admission-restored.log) |
| mutations | payload-disposal | 101 / 0 | [patch](evidence/mutations/payload-disposal.patch), [mutant](evidence/mutations/payload-disposal.log), [restored](evidence/mutations/payload-disposal-restored.log) |
| mutations | queued-drop-uncontained | 0 / 0 | [patch](evidence/mutations/queued-drop-uncontained.patch), [mutant](evidence/mutations/queued-drop-uncontained.log), [restored](evidence/mutations/queued-drop-uncontained-restored.log) |
| mutations | record-read-as-absence | 101 / 0 | [patch](evidence/mutations/record-read-as-absence.patch), [mutant](evidence/mutations/record-read-as-absence.log), [restored](evidence/mutations/record-read-as-absence-restored.log) |
| mutations | server-ambient-executor | 101 / 0 | [patch](evidence/mutations/server-ambient-executor.patch), [mutant](evidence/mutations/server-ambient-executor.log), [restored](evidence/mutations/server-ambient-executor-restored.log) |
| mutations | server-early-permit-release | 101 / 0 | [patch](evidence/mutations/server-early-permit-release.patch), [mutant](evidence/mutations/server-early-permit-release.log), [restored](evidence/mutations/server-early-permit-release-restored.log) |
| mutations | server-payload-disposal | 101 / 0 | [patch](evidence/mutations/server-payload-disposal.patch), [mutant](evidence/mutations/server-payload-disposal.log), [restored](evidence/mutations/server-payload-disposal-restored.log) |
| mutations | sqlite-read-as-absence | 101 / 0 | [patch](evidence/mutations/sqlite-read-as-absence.patch), [mutant](evidence/mutations/sqlite-read-as-absence.log), [restored](evidence/mutations/sqlite-read-as-absence-restored.log) |
| mutations | two-physical-slots | 101 / 0 | [patch](evidence/mutations/two-physical-slots.patch), [mutant](evidence/mutations/two-physical-slots.log), [restored](evidence/mutations/two-physical-slots-restored.log) |
| boundary-mutations | capture-drop-uncontained | 101 / 0 | [patch](evidence/boundary-mutations/capture-drop-uncontained.patch), [mutant](evidence/boundary-mutations/capture-drop-uncontained.log), [restored](evidence/boundary-mutations/capture-drop-uncontained-restored.log) |
| boundary-mutations | definite-record-store | 101 / 0 | [patch](evidence/boundary-mutations/definite-record-store.patch), [mutant](evidence/boundary-mutations/definite-record-store.log), [restored](evidence/boundary-mutations/definite-record-store-restored.log) |
| boundary-mutations | definite-sqlite-write | 101 / 0 | [patch](evidence/boundary-mutations/definite-sqlite-write.patch), [mutant](evidence/boundary-mutations/definite-sqlite-write.log), [restored](evidence/boundary-mutations/definite-sqlite-write-restored.log) |
| boundary-mutations | queued-drop-uncontained-strengthened | 101 / 0 | [patch](evidence/boundary-mutations/queued-drop-uncontained-strengthened.patch), [mutant](evidence/boundary-mutations/queued-drop-uncontained-strengthened.log), [restored](evidence/boundary-mutations/queued-drop-uncontained-strengthened-restored.log) |
| boundary-mutations | ready-cleanup-fault-acknowledged | 101 / 0 | [patch](evidence/boundary-mutations/ready-cleanup-fault-acknowledged.patch), [mutant](evidence/boundary-mutations/ready-cleanup-fault-acknowledged.log), [restored](evidence/boundary-mutations/ready-cleanup-fault-acknowledged-restored.log) |
| boundary-mutations | secret-read-as-absence | 101 / 0 | [patch](evidence/boundary-mutations/secret-read-as-absence.patch), [mutant](evidence/boundary-mutations/secret-read-as-absence.log), [restored](evidence/boundary-mutations/secret-read-as-absence-restored.log) |
| boundary-mutations | server-call-uncontained | 101 / 0 | [patch](evidence/boundary-mutations/server-call-uncontained.patch), [mutant](evidence/boundary-mutations/server-call-uncontained.log), [restored](evidence/boundary-mutations/server-call-uncontained-restored.log) |
| boundary-mutations | server-capture-drop-uncontained | 101 / 0 | [patch](evidence/boundary-mutations/server-capture-drop-uncontained.patch), [mutant](evidence/boundary-mutations/server-capture-drop-uncontained.log), [restored](evidence/boundary-mutations/server-capture-drop-uncontained-restored.log) |
| boundary-mutations | server-queued-drop-uncontained-strengthened | 101 / 0 | [patch](evidence/boundary-mutations/server-queued-drop-uncontained-strengthened.patch), [mutant](evidence/boundary-mutations/server-queued-drop-uncontained-strengthened.log), [restored](evidence/boundary-mutations/server-queued-drop-uncontained-strengthened-restored.log) |
| boundary-mutations | server-ready-cleanup-fault-acknowledged | 101 / 0 | [patch](evidence/boundary-mutations/server-ready-cleanup-fault-acknowledged.patch), [mutant](evidence/boundary-mutations/server-ready-cleanup-fault-acknowledged.log), [restored](evidence/boundary-mutations/server-ready-cleanup-fault-acknowledged-restored.log) |
| payload-boundary-mutations | capture-payload-disposal | 101 / 0 | [patch](evidence/payload-boundary-mutations/capture-payload-disposal.patch), [mutant](evidence/payload-boundary-mutations/capture-payload-disposal.log), [restored](evidence/payload-boundary-mutations/capture-payload-disposal-restored.log) |
| payload-boundary-mutations | server-capture-payload-disposal | 101 / 0 | [patch](evidence/payload-boundary-mutations/server-capture-payload-disposal.patch), [mutant](evidence/payload-boundary-mutations/server-capture-payload-disposal.log), [restored](evidence/payload-boundary-mutations/server-capture-payload-disposal-restored.log) |

The [inline adapter restoration](evidence/inline-baseline/inline.patch) is a separate grouped offload comparison, not26 additional probes. It compiles and fails all8 named actual current-thread heartbeat witnesses: SQLite2, record/secret5, audit1. [Inline SDK RED](evidence/inline-baseline/inline-heartbeat-red.log), [inline server RED](evidence/inline-baseline/inline-server-heartbeat-red.log), and [restored8 GREEN](evidence/inline-baseline/restored-heartbeat-green.log) report tick counts: inline1→1; restored1→39/40 over the OS-held120ms interval. These are sampled tick counts, not heartbeat jitter quantiles.

## Local measurements

[Environment/settings and clock scope](evidence/benchmark-environment.txt). All three modes have21000 SQLite and1000 OAuth samples: 1000 per scenario. Production-library integration targets exclude cfg(test) probes. SQLite fixtures are legal32/256/2048 root lifetimes (zero spawns), open/closed history, and empty; raw metadata records exact counts/body sizes. No2048-reservation claim; retained-request cap is128.

Debug Linux tmpfs, SQLite DELETE/FULL, scripted zero-delay OAuth with no real network. Four scheduled writers through one store and four independent OAuth lanes are configured caller counts, not simultaneous-in-one-poll or measured in-flight claims. Elapsed starts at each task/flow first poll/initiation, excluding preceding scheduler waiting. Inline measurements may therefore hide scheduler delay. Raw caller latency includes clone/admission/queue/I/O/join; no phase timings or throughput claim. Median is the mean of two middle values; p95/p99 use nearest rank. 1000 samples limit tail resolution.

The worker path adds an owned-input clone, admission and blocking-task handoff. One slot serializes complete encoding/I/O/captured cleanup through one instance; the inline SQLite baseline retains only its existing connection mutex, with encoding before that mutex. Component timings were not measured, so comparisons attribute no separate quantified cost. The combined path adds observable caller latency, especially small operations and queued writers. The change establishes bounded physical ownership and runtime responsiveness, and does not claim faster end-to-end calls. Every measured regression remains in the table. Initial worker precedes cfg(test)-only witness strengthening; final worker repeat is the current restored-source measurement. The baseline reconstructs inline delegation over the same synchronous state algorithms/harness, rather than claiming an original-base binary.

| Roots/history; scheduled callers; operation | Inline median / p95 / p99 µs | Initial worker µs | Final worker repeat µs |
| --- | --- | --- | --- |
| 0 / false / 1 / write | 28.5 / 92.7 / 204.2 | 73.0 / 126.4 / 249.2 | 77.8 / 115.2 / 181.6 |
| 0 / false / 1 / read | 10.5 / 23.0 / 79.3 | 42.1 / 70.2 / 136.2 | 31.0 / 69.5 / 148.0 |
| 0 / false / 4 / write | 93.3 / 195.5 / 352.3 | 209.8 / 346.4 / 441.0 | 185.7 / 357.8 / 468.6 |
| 32 / false / 1 / write | 156.2 / 313.6 / 543.9 | 225.8 / 319.2 / 470.2 | 246.4 / 350.6 / 522.6 |
| 32 / false / 1 / read | 80.6 / 161.7 / 324.3 | 157.2 / 201.0 / 294.5 | 147.8 / 216.7 / 271.8 |
| 32 / false / 4 / write | 232.7 / 389.0 / 574.7 | 728.1 / 1266.1 / 2087.7 | 690.8 / 1104.7 / 1309.7 |
| 32 / true / 1 / write | 346.3 / 572.7 / 728.5 | 448.3 / 647.3 / 1025.1 | 443.5 / 611.5 / 788.4 |
| 32 / true / 1 / read | 163.4 / 255.5 / 425.6 | 243.3 / 352.0 / 527.6 | 240.6 / 326.2 / 485.8 |
| 32 / true / 4 / write | 563.4 / 698.2 / 1028.3 | 1256.3 / 2030.9 / 2490.3 | 1228.9 / 1921.7 / 2120.4 |
| 256 / false / 1 / write | 1011.8 / 1383.3 / 1868.9 | 1142.3 / 1418.6 / 2303.7 | 1123.8 / 1348.5 / 1576.6 |
| 256 / false / 1 / read | 528.0 / 762.6 / 1140.1 | 602.9 / 798.4 / 911.5 | 586.3 / 761.0 / 884.1 |
| 256 / false / 4 / write | 1155.6 / 1964.3 / 2127.7 | 3295.1 / 5097.9 / 6117.4 | 3256.5 / 4763.6 / 5216.2 |
| 256 / true / 1 / write | 2532.1 / 3247.2 / 4307.8 | 2641.8 / 3211.0 / 4252.4 | 2536.3 / 2927.7 / 3327.3 |
| 256 / true / 1 / read | 1221.1 / 1579.1 / 2390.8 | 1276.4 / 1640.8 / 2015.7 | 1228.9 / 1477.8 / 1596.9 |
| 256 / true / 4 / write | 4391.2 / 4721.2 / 5719.7 | 7489.5 / 11078.0 / 12023.8 | 7615.1 / 11691.9 / 15802.3 |
| 2048 / false / 1 / write | 8077.5 / 11483.9 / 16656.9 | 8586.7 / 10268.3 / 13113.9 | 8469.5 / 10213.3 / 12275.4 |
| 2048 / false / 1 / read | 4241.4 / 6282.8 / 9312.9 | 4558.7 / 5790.8 / 8290.9 | 4325.1 / 5339.6 / 6869.7 |
| 2048 / false / 4 / write | 10072.5 / 14840.6 / 18559.1 | 23839.2 / 35862.8 / 39347.2 | 24224.7 / 37904.7 / 49479.6 |
| 2048 / true / 1 / write | 19043.6 / 22145.5 / 27118.2 | 21190.9 / 27650.0 / 37180.6 | 21226.0 / 25845.6 / 30163.1 |
| 2048 / true / 1 / read | 9331.4 / 10838.9 / 13586.8 | 9587.5 / 13842.6 / 20040.4 | 10121.2 / 12506.6 / 16894.5 |
| 2048 / true / 4 / write | 20530.9 / 35735.5 / 36436.9 | 60315.5 / 89249.4 / 100308.2 | 55895.6 / 88657.4 / 93818.4 |
| Composed OAuth,4 lanes, authorize/callback/bearer/rejected refresh | 834.4 / 4915.9 / 7009.3 | 2161.2 / 2778.7 / 3038.6 | 2341.4 / 3013.5 / 3324.5 |

Raw datasets: [sqlite-inline](evidence/sqlite-inline.csv.gz), [sqlite-worker](evidence/sqlite-worker.csv.gz), [sqlite-worker-repeat](evidence/sqlite-worker-repeat.csv.gz), [oauth-inline](evidence/oauth-inline.csv.gz), [oauth-worker](evidence/oauth-worker.csv.gz), [oauth-worker-repeat](evidence/oauth-worker-repeat.csv.gz). [Machine-readable quantiles](evidence/benchmark-summary.json).

## Final checks

The clean-source guarded runner records actual tested HEAD, commands, environment and results in `evidence/final-gates/manifest.json` after execution. Historical green logs are retained separately and do not stand in for current-head gates. Linux checks do not establish Windows/macOS/cross-feature execution. Atlas validation covers118 IDs, parents/cycles, source and diagram links, plus edited Markdown paths/anchors. Mermaid syntax was inspected; its external atlas browser/rendering was unavailable and was not claimed passed.
