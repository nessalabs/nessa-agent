# #627 final independent review brief — fresh round1

Checkout: /workspace/nessa-agent-bounded-persistence; branch codex/627-bounded-persistence.
BASE: root-selected actual latest main (currently d55de88d9c6f5794b225da05b1055c511b9dba74).
HEAD:a8e2f1397220cb84750ef4450b16bcd4314045c3 (clean frozen; actual corrected gates complete). No prior formal #627 review round occurred.
Read AGENTS.md, CODING_STANDARDS.md fully and actual architecture/DI/module maps.
Review entire cumulative base/head change and its relationships; source/evidence
reasoning is not personally executed tests. Root owns exact-head CI and merge.

The change moves SQLite ownership read/write, five FileRecords operations and
FileAuthorizationAudit append/sync off ordinary Tokio polling. One optional,
default-off nessa-local-storage::physical_operation implementation owns admission,
originating executor and captured cleanup; every adapter owns an independent Worker.
Worker/Admission/Interrupted are infrastructure only, not SDK product reexports.
Admission captures Handle before semaphore wait; consuming submit hands off the
job synchronously before returning its awaitable. Job owns real operation capture
and permit through queued/run/captured cleanup; result waiter loss detaches it.
Call, captured Drop, never-started Drop and Ready downgrade/disposal boundaries
are separate. Adapter state/algorithms, typed mappings and application/domain
revision/OAuth/settlement authority stay with existing owners. Constructors/disk
formats unchanged. No configurable capacity/global path lock/retry/controller.

Required challenge: admission precedes actual input clone construction; abandoned
caller cannot release physical slot; ordinary read faults never imply absence;
secret mutation interruption remains Unknown; audit failure never becomes Ack;
origin executor capture supports queued Send future resumed on a plain thread;
independent instances progress; central primitive is actually selected in CI.
Compare canonical 26-row table/state atlas/API docs with source and concrete
adapter and central witnesses. Verify default-off NORMAL dependency tree has no
Tokio; feature-on and combined six-package logs name all14 central tests.

Evidence must be filled with final public/local manifests. Historical393 two-helper
proofs/timings are archived, not current shared-source validation. Current26
single-site mutations require compiled assertion failure and exact restored green.
Audit probe22 resumed after interrupted orchestration: actual RED log retained;
original process exit101 reconstructed, not newly captured. Failed shutdown
assumption and ENOSPC compile are non-passes, preserved. Already queued Tokio
blocking work may complete after shutdown; actual stopped-executor adapter
submission proof is distinct from central private queued-abort/detached proof.
No actual adapter detached caller loss inside synchronous rejected submission is
claimed. Source-included test fixture contains capture observations/gates, no
copied admission/catch/join policy.

Measurements: unchanged7 legal SQLite root/history fixtures,21k per-call rows and
1k scripted OAuth flows per phase; current/reconstructed-inline/restored phases
66k rows total. Existing per-call clock starts at first poll and excludes earlier
scheduler wait. Additional wall observer measures each existing4writer batch
before spawning through ALL completion, then CSV writes;250 batches PER fixture,
1750 per phase/5250 all phases, not1000 independent wall samples. Scenario state
assertion and observer-only instrumentation changes have precise source hashes.
Debug local tmpfs, two runtime workers/eight blocking workers, no real-network
latency claim; report overhead/regressions/alltails, not favorable percentile only.
Observed restored medians: empty write75.18us versus inline27.91us;2048-open
write8.881ms versus7.684ms; four scheduled-call per-call24.738ms versus7.851ms;
full batch wall35.338ms versus16.245ms (N250); scripted OAuth2.466ms versus0.874ms.
Inspect the complete all-scenario table and latency/throughput tradeoff, including
single-slot serialized validation/encoding/physical work. No speedup is claimed.
Responsiveness is independent OS-gated actual adapter heartbeat evidence, not a
benchmark timing threshold. Shared offload-rule reversion is one source variant
exercised by eight distinct adapter witnesses, not eight source mutations.

Limits: origin must progress for reliable completion; shutdown can conservatively
fail or queued work may still succeed. No cross-instance/path exclusion or shutdown
drain. Physical-frame double faults may abort; external panic hooks not suppressed.
Returned/discarded output Drop after Job returns is outside permit lifetime.
Audit retains two writes and can leave a partial line. SDK/server actual MSRV1.89;
record tested toolchain1.99 without claiming an unrun1.89 build. External atlas
browser/Mermaid rendering unavailable; actual markdown-it one26-row table proof
and118-page source/link/metadata checks are distinct from hosted render approval.
Assess gate17 applicability on actual changed behavior; no browser pass is claimed.

Reviewers complement domain/API/docs agreement versus adversarial lifetime/data/
measurement evidence. No source edits or concurrent Cargo without root allocation.
Reports give exact head/base, concrete severity/disposition, checked relationships,
actual execution versus inspected logs, unverified limits and approval decision.

Full isolated SDK and actual combined six-package suites passed at original209.
Corrected a8 source differs only by removal of .ok() in cfg(test) held-admission
setup; production/benchmark/mechanic rules are identical. Nine archived artifacts
were formatted, with exact original raw gzip bytes/hash provenance retained.
Original209 frontend formatting and Clippy non-passes are preserved. Currenthead
focused SDK21/all-targetClippy/rustdoc/architecture/format/scripts checks must be
filled after completion; hosted CI runs full suites on actual correctedhead.

Actual current-head local gate manifest: /tmp/627-corrected-final-gates/manifest.json.
Seven gates have observed exit0: format; actual SDK ownership21; six-package
all-targetClippy; SDK docs (rustdoc-Dwarnings); architecture tests/check;
frontendformat. Local script summary348tests/346pass/0fail/2skip is preserved,
but final process exit was not retained across agent pause and is NOT a local
completed-command pass. Replacement actual hosted frontend job112959792534,
run37670126367, is SUCCESS and completed pnpmfrontend:check through scripts,
all subsequent checks and build. Raw joblog /tmp/652-currentfrontend-job.log.
HostedplatformCI still active; root verifies all requiredchecks beforemerge.

Original209 manifests/logs: /tmp/627-consolidated-final-gates/. FullSDK984unit
(13ignored),401app(4ignored),212domain,65infra(3ignored),38doctests. Exact
sixpkg suite passed and central-ci-presence.json confirms all14 actualnames.
The nine archive-format raw originals and semantic/source provenance are on
a8 in verification/bounded-persistence/evidence/consolidated.
No formal627 review rounds have occurred; this is freshR1.
PR652 is draft. Reports /tmp/627-r1-contracts-report.md and
/tmp/627-r1-adversarial-report.md; source read-only. No concurrent Cargo or
source edits; if a concrete reproduction needs tools/building request allocation
from root, rather than changing the frozen checkout.
