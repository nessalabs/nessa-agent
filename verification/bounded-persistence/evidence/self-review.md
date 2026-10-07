# Implementer review: bounded physical persistence #627

Base: f1682e8a763646c6c3e73dee73c1c47987e2cbae. The final handoff records the clean
candidate commit and final-gate tested commit; the source hash manifest binds the
implementation, tests and documentation. This is implementer review, not either
independent review or permission to merge. Historical RED sources were dirty over
the base and are identified separately in evidence provenance.

## Dimensions and agreement across layers

| Dimension | Inspected relationship, enforcing code and evidence |
| --- | --- |
| Authority and identity | Existing OwnershipStore/AuthorizationRecords/AuthorizationAudit remain application-owned ports. No new public authority, registry or mutable domain state. Each adapter's Worker owns only that instance's physical admission; Job owns its admitted permit. Coordinator revision fence and OAuth owner generation/reply/settlement logic are untouched. Existing newer-seal regression and normal composed OAuth generations remain accepted. |
| Domain invariants | Snapshot DTO conversion and OwnershipGraph::restore validation are the existing algorithm, now inside OwnershipState's physical call. Codec fields/formats and owner/domain validation are unchanged. Roundtrip, corrupt body, unsupported schema and domain fixtures check accepted/rejected restoration; large benchmark snapshots are verified legal with zero spawns. |
| Admission and concurrency | Handle captured before waiting; semaphore admission before large input clone and spawn; one job per instance. Cancellation before admission submits nothing. Queued/executing/capture-cleanup jobs retain the same slot after caller loss. Actual before/after/Input Drop gates and OS watchdogs cover SQLite, records, secrets and audit; independent-instance witnesses exclude a global promise. Plain-thread resume and no-runtime/shutdown tests cover executor capability. |
| Failure and cleanup | Physical state owns the connection/directory/audit mutex. Job catches call while still owning capture, separately contains capture Drop, contains remaining capture in neverstarted Drop, and downgrades Ready when capture cleanup fails. Caught panic payloads are forgotten without inspection. Typed Uncertain/Unavailable/Unknown/AuditFailure preserve effect uncertainty; landed-effect tests reload real data. Mutation removals have separate outcomes in the evidence manifest. |
| Observations and restoration | NoRows/NotFound alone produce absence. Poison, read/decode failure, worker faults and capture faults do not fabricate absence. Concurrent physical store/delete/replacement evidence observes actual bytes after cleanup. This helper does not settle whole coordinator attempts or add revision authority. Existing application/domain restoration and whole-attempt debts are unchanged and outside this issue. |
| Audit | Existing server/action/intent/generation/resource/phase JSON fields and append/sync remain inside actual AuditState. Normal record and sink failures, poison, detached caller ordering and Ready/capture faults are checked. JSON and newline remain separate writes: no atomic-line claim, and interruption can leave partial bytes. Composed benchmark scans audit/record bytes for the known token fixtures. |
| Representation boundaries | DTO/JSON/sealed formats, permissions, crypto and existing durability algorithms remain unchanged. All synchronous decoding/encoding/key/file work is in the physical operation, not merely the final write. Existing sealed/tamper and storage corruption tests remain exercised. Public constructors stay synchronous/runtime-free; no-runtime first poll is typed pre-effect refusal. Linux local tests do not establish cross-target execution. |
| Resource bounds | One admitted/submitted blocking job per adapter instance, including blocking-pool waiting and capture cleanup. Async caller waiters are not a bounded total caller count; their borrowed large input is not copied before admission. No cross-instance/process/path bound or live runtime-independent drain. Production-library measurements disclose root/history/body sizes, actual instance/caller counts and wrapper overhead. |
| Public surface and organization | No public signatures/dependencies or scheduler API change. SDK helper has one consumer, server helper two, scoped to their independent instance ownership; no duplicated domain decision. Test siblings, module maps, design table and minimal storage atlas follow the owning features. SDK missing-docs/doc checks and six-package architecture/Clippy results are recorded for the tested source. |

## Canonical gates

Typed failures and conservative uncertainty (1,5,7,16) are enforced by actual
adapter tests and wrong-result mutations. Names, feature/layer scope, organization
and port DI (2,3,4,8,9,13) were traced through each method, private helper, real
state and consuming application owner. No raw string error parsing or new
identity/representation rule was introduced (11,12). Guarantees name table
witnesses (10); the initial18-row table preceded executable changes, and runtime
and capture cases were added before their implementation (15). Exact checks,
command/environment/source identity and limitations establish gates6/14. Gate17
has no product UI change here; atlas source/metadata/link checks ran, while its
external browser rendering tool was unavailable. This is disclosed, not a claimed
browser pass.

## Explicit limits

Once Job returns an output, its destructor is outside the permit lifetime, whether
delivered or discarded after caller loss. Ready output disposal after capture
cleanup faults remains inside the explicit containment boundary. A
process abort caused by multiple faults during the physical operation's own local
guard unwind is unrecoverable. These catches do not suppress external panic hooks
or ensure process-wide secret-safe panic-hook output. Runtime shutdown can prevent
completion; reliable progress assumes the captured originating executor remains
alive. Physical ordering is per instance and does not imply multi-method
transactions, fairness beyond the underlying semaphore, global serialization,
shutdown draining or atomic audit lines.

Measurements use local Linux tmpfs, debug profile, production libraries without
cfg(test) probes, SQLite DELETE/FULL, scripted zero-delay OAuth network/callback
inputs, 1000 samples and scheduled concurrent callers. They are caller latency,
not isolated disk/crypto phases, pre-first-poll scheduler waiting,
simultaneous-in-one-poll readiness, production
network performance or stable tail estimates. Benchmark medians/p95/p99 and any
regressions remain visible. Windows/macOS/cross-feature builds and external atlas
rendering were not run.
