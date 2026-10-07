# #627 / PR652 independent R2 contracts review

Checkout: `/workspace/nessa-agent-bounded-persistence`.
BASE: `d55de88d9c6f5794b225da05b1055c511b9dba74`.
HEAD: `616a5fb9b07b81e30f23ff3282c1c60722fbb306`.
Tree clean at review. Source was read-only: no edits, Cargo invocation, rebuild,
mutation, or personally executed Rust test. This is a fresh cumulative base/head
review, including the correction from a8, rather than inherited R1 approval.

Decision: **approve reviewed code/contracts**. No blocker, major, minor or nit
findings. Hosted current-head required checks remain root-owned merge gates; this
review does not represent pending CI as a pass.

## R1 closure and neighboring orders

R1-C1 major is closed. `Admission::submit` catches the synchronous invocation of
its actual captured executor, forgets the caught payload immediately before the
async observer is constructed, and moves Result<JoinHandle,Interrupted> into that
observer. Existing eight method-specific outcome conversions are unchanged.
`spawn` first constructs Job with capture and permit and transfers that Job into
Tokio. Catching executor-entry failure does not release or reclaim a queued Job,
claim rollback, or fabricate absence. The inspected actual OS log at exact616
records first-worker EAGAIN, typed interruption, a later origin kick and success;
the manual test source asserts no effect at interruption, successor pending,
actual file bytes/once-only count, capture-cleaning exclusion and release before
successor. Portable thread_name_fn tests cover both polled and unpolled observers
with a faulting payload destructor, without pretending their poisoned pool can
recover. ManuallyDrop is explicitly test-only and child-process bounded.

Checked adjacent cases: no runtime before admission; cancellation while waiting;
unused acquired admission; live origin resumed on plain thread; stopped origin
before handoff; successful synchronous handoff with unpolled observer loss;
queued/running/capture-cleaning caller loss; genuine private queued abort;
ordinary operation panic before and after real effect; operation panic plus
capture fault; Ready plus capture fault, disposal fault, and detached observer.
Job takes its operation Option before automatic permit-field destruction; call
and capture destruction are independent catches. Each catch forgets its payload.
Ready downgrade disposes output under the permit before returning Interrupted.
Returned output destruction after Job completion has a separately disclosed limit.

R1-C2 minor is closed. The README now points to the maintained public record;
the empty consolidated pending stub is deleted. I inspected the Git object for
published33894ed0: its manifest names exact616 and base, all11 local results,
manual OS proof and archive hash while saying R2/current hosted CI are pending.
It separates original209 full suites, a8 observed checks and hosted success from
current616 focused execution. An incomplete overall approval flag is truthful,
not an empty placeholder presented as completed verification.

## Relationships, owners and nine review dimensions

| Dimension / related facts checked | Enforcers and regression evidence inspected |
| --- | --- |
| Authority and identity | Worker owns private independent semaphore; no Clone, path/global authority or public abort handle. Admission privately couples originating Handle and permit and is consumed by submit. Job retains operation/state/input with that same permit. Independent central/SDK/records instance tests distinguish instance exclusion from path exclusion. Application coordinator and OAuth owner/domain authority are unchanged. |
| Domain invariants | Production diff moves existing adapter effects to private state methods; no domain or application decision owner is relocated. Coordinator::commit_snapshot still owns write_order and revision comparison before store.write; resume_from_store still applies OwnershipGraph::restore. Existing older-snapshot regression remains the row16 witness. Snapshot DTO/parser and OAuth phase/generation/settlement algorithms stay unchanged. |
| Admission and concurrency | All eight adapter methods await admit before their actual owned clone and synchronous submit. Origin Handle is captured before semaphore await, not re-resolved on plain-thread resumption. Central input/cancellation/unused/unpolled tests and SDK/records/audit clone observations enforce those distinct stages; queue, execution and capture cleanup retain physical ownership after caller loss. |
| Failure and cleanup | Immediate executor-entry catch/forget, separate Job call and Drop catches, never-started cleanup and Ready-output disposal are the one shared implementation. New two compiled RED/restoredGREEN mutation records exercise catch and immediate forget. Existing actual adapter pre/post-effect, stopped-executor, poison and captured-input witnesses distinguish typed result from physical effect. No submission-failure inference releases a retained Job. |
| Observations and restoration | SQLite NoRows alone returns empty; query/decode rejection and mutex poison/interruption remain typed. Records NotFound retains its existing absence meaning; failure is Unavailable. Secret interrupted publication/deletion is Unknown, with recovery load checking actual disk state. Corrupt/illegal SQLite bodies and foreign schema remain unchanged on refusal. OAuth owner's existing .ok().flatten probe policy is not moved into adapter or claimed fixed here. |
| Audit | The actual AuthAuditRecord clone retains server/action/intent/generation/resource/phase through append/sync. Pre-effect, post-effect, poison, Ready-cleanup fault and caller-loss tests show AuditFailure can coexist with real bytes; no fake ack follows interruption. Same-instance B append waits through A's physical cleanup. Application audit attribution and mandatory sink policy remain their existing owners. |
| Representation boundaries | SQLite schema/DTO conversion and records JSON/sealing/key/randomness/write/delete algorithms are preserved; actual round-trip, tamper/private-file and unchanged-body tests remain. Encoding/query/decode and native effects run inside admitted state closure. Constructors remain synchronously opening/constructing adapters; no migration, transport DTO or product schema is added. |
| Resource bounds | Bound is one queued/running/cleaning physical Job per independent instance, not one waiter or a global byte/history cap. Large borrowed-input copies follow admission. Waiting future cancellation creates no owned job/capture; detached observation does not free physical capacity. Normal clone/validation/encoding/I/O/cleanup serialization and its latency cost are disclosed. |
| Public surface and organization | Default-off local-storage optional feature owns Worker/Admission/Interrupted and private Job. SDK/server explicitly opt in; SDK product surface does not reexport mechanics. One source-included cfg(test) fixture owns OS gates/real capture observations, with no semaphore/executor/catch policy copy. Source/tests/benchmarks/maps/table/atlas use the owning feature/layer paths. No crate, configurable capacity, controller, retry/ledger or public test switch is introduced. |

Concrete contradictions challenged: an interruption claiming no file effect;
Ready acknowledgement surviving failed captured cleanup; failed read becoming
empty; caller loss releasing a queued Job's slot; admission on executorB after
waiting on executorA; old snapshot overwriting newer published revision; audit
failure becoming success because bytes or cleanup are present. Current code and
inspected tests retain these as separate facts. Positive paths still accept legal
snapshot reopen and secret publication/load/delete; independent instances progress.

## Standards and gates

Read AGENTS.md and CODING_STANDARDS.md, architecture and typed-DI guidance;
consulted actual SDK/subagent/storage/server maps, affected repository map, API
comments, design table and SDK/MCP/storage atlas. Reviewed full cumulative active
production/config/test/docs changes, central and adapter tests, shared fixture,
production-library benchmark harnesses, and separately the a8→616 correction.
Archived source copies are provenance, not additional active production owners.

All17 gates assessed: 1 typed infrastructure/adapter conversions; 2 plain physical
operation naming; 3 infrastructure boundaries; 4 scope-preserving production diff;
5 finite refusal/call/cleanup regressions; 6 exact616 checks below; 7 conservative
outcomes/no fake success; 8 maps/test/fixture placement; 9 existing injected
OwnershipStore/AuthorizationRecords/AuthorizationAudit seams unchanged; 10 claims
name tests/types and progress limits; 11 private construction and exhaustive typed
matches; 12 durable domain identities unchanged; 13 one implementation plus
load-bearing mutations; 14 optional/default-off and actual combined feature
selection/platform evidence separated by checkpoint; 15 canonical27-row ordering
table including row27 before correction; 16 no invented effect precision/retry
machinery; 17 backend physical scheduling changes with no UI/MCP wire schema
change, no inferred desktop browser pass. Root owns any broader scripted product
integration requirement and current hosted completion.

## Personally executed versus inspected

Personally executed read-only at616:

- Status/head and cumulative/correction source diffs.
- Recomputed all30 final source-manifest entries, including four null absent old
  helper paths: no mismatch. Recomputed all25 handoff artifact hashes: no mismatch.
- Ran tracked doc-validator: exit0,118 atlas metadata/source/link pages, canonical
  relative paths, test-name existence and default-off manifest syntax. Counted27
  canonical rows. This does not render Mermaid or establish the Cargo graph.
- Matched all15 current portable central names to actual PASS lines in the exact
  six-package filtered log; manual Linux witness remains explicitly ignored there.
- Inspected exact published33894ed0 manifest through git show and confirmed clean
  source identity. I did not personally fetch hosted results or execute Cargo.

Inspected execution evidence, not personally run:

- Frozen616 manifest: all11 finite commands have observed exit0 (fmt, central15,
  SDKownership21, records20, audit7, combined central15 presence, six-package
  all-targetClippy -Dwarnings, SDKdocs, architecture tests/check, frontendformat).
  Owned runner completion is recorded. Combined filtered command is not a full
  current six-package suite.
- Exact616 manual OS binary baseline/refusal/repeat exit0 with binary/shim/log
  hashes and actual retained Job/file/capture assertions. Two new mutation RED101
  and restored0 records identify helper58b...; final8634... differs only the
  archived rustfmt line wrap. No exact616 mutation recompilation is invented.
- Older209 full isolated SDK suites and six-package run, a8 all10 hosted checks
  and14 central cases on Linux/macOS/Windows are checkpoint-specific support.
  They are not relabeled full616 execution. Current run37675120022 was active in
  the supplied handoff; root verifies all required current checks before merge.
- Old26 single-site mutation/provenance inventory and disclosed reconstructed
  audit22 exit; failed queued-shutdown assumption and ENOSPC compile remain
  non-passes. Pre-R1 66000 per-call/flow and5250 batch rows are not exact616
  latency evidence. Harnesses and all-scenario summary retain regressions,
  N250 wall batches, per-call first-poll timing and debug/Linux/tmpfs/zero-network
  limitations. No speedup or production latency is asserted.

## Exclusions and limits

No personally executed Cargo, MSRV1.89, macOS/Windows or hosted CI; local compiled
checks use rustc1.99. No external atlas/Mermaid browser rendering or desktop
Chromium/WebKit scripted verification is claimed from source/link checks.
No cross-instance/path/process lock, bound on waiting callers or aggregate input
bytes, runtime-independent completion, bounded OS recovery, guaranteed shutdown
queue drain, general panic-hook suppression, physical-frame double-fault recovery,
post-Job output destructor lifetime or atomic audit JSON/newline. Unchanged broader
custom-adapter/domain command histories, provider deadlines, socket receipts and
bulk cleanup were inspected only where they consume these unchanged ports; this
focused review does not assert their repository-wide absence of defects. Not every
archived historical file/log line was reviewed as current active implementation.
