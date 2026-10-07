# #627 / PR652 fresh R2 review brief

Checkout: /workspace/nessa-agent-bounded-persistence.
BASE d55de88d9c6f5794b225da05b1055c511b9dba74.
HEAD616a5fb9b07b81e30f23ff3282c1c60722fbb306, clean frozen.
Read AGENTS.md/CODING_STANDARDS.md fully plus actual architecture/DI/module maps.
Review entire cumulative base/head diff and adjacent paths, and R1 correction
from a8e2f1397220cb84750ef4450b16bcd4314045c3. This is fresh round2, preserving
R1 findings/history; no prior approval may substitute for review.

Behavior: move SQLite ownership read/write, five FileRecords methods and audit
append/sync off normal Tokio polling. One optional/default-off local-storage
physical_operation implementation owns each independent adapter instance slot,
origin Handle captured BEFORE semaphore waiting, synchronous operation handoff,
actual capture cleanup and contained failures. Worker/Admission/Interrupted are
infrastructure types with private fields; no SDK product reexport, new crate,
controller, configurable capacity, global/path lock or public abort/probes.
Actual adapter state and typed outcome mapping stay with their existing owners.
One cfg(test)-only source-included fixture shares real capture/clone observations
and OS gates, not semaphore/executor/catch policy. Both old helper copies removed.

R1 majorC1 was a real OS refusal of the first blocking thread: baseline0/refusal101/
restored0 at /tmp/627-r1-spawn-refusal. Tokio queues Job before thread creation and
panics synchronously, previously bypassing all8 conservative adapter maps. R1
minorC2 was canonical README's empty pending gate stub. Read both R1 reports:
/tmp/627-r1-contracts-report.md and /tmp/627-r1-adversarial-report.md.
Correction616 catches self.spawn synchronously, immediately forgets caught panic
payload BEFORE constructing observer, and stores Result<JoinHandle,Interrupted>.
Existing Job owns operation+permit; failure does not imply no effect, rollback or
slot release. Origin may later execute queued work. Zero-worker shutdown is NOT
promised to drain it. Adapter maps/algorithms, disk formats, app/domain revision/
OAuth/settlement authority and benchmark harnesses unchanged.

Canonical table now27 rows. Fifteen portable mechanic tests plus one explicitly
ignored Linux manual witness. Portable legitimate RuntimeBuilder.thread_name_fn
fault child establishes synchronous typed interruption, polled/unpolled observer
and immediate payload forget; ManuallyDrop isolates Tokio poolpoison, NO recovery
claim from that fixture. Actual Linux pthread EAGAIN probe establishes typed
interruption beforeeffect, pending successor, independent same-origin blocking
kick, exactly-once actual file effect, captureDrop holds slot, release-before-
successor. No production test switch or GCC CI dependency. New catch/forget rules
each actual compiled RED/restoredgreen. Exact newmutation helper58b... versus
frozen8634... differs only finalrustfmt; patch/provenance preserved.

Challenge all original lifetime boundaries too: real clone constructor AFTER
admission, caller/unpolled wait loss, independent instances, queued neverstarted
Drop, call/capture cleanup separation, Ready downgrade/disposal, forgotten panic
payloads and captured origin for Sendfuture resumed on plainthread. No failed
storage read invents absence; secret publication/deletion interruption remains Unknown; secret read interruption is Unavailable; audit never
fakeAck. Limits: origin progress; no cross-instance/path lock or shutdown drain;
physical-frame doublefault may abort, external panic hooks not suppressed;
returned output Drop after Job ends outside permit lifetime; existing possible
partial audit line. SDK/server MSRV1.89; actuallocaltoolchain1.99, no unrun MSRV
claim. Gate17 applies to actualbehavior: backend storage change, no UI/MCPwire
schema change or desktopbrowserpass claimed. External atlas/Mermaid rendering
unavailable; actual markdown-it one27-row table and118-page source/link checks
are distinct evidence.

Evidence/provenance: old26 variants were performed on pre-R1 shared helperae9...,
with exact patches/runtime expectations/byte-restoredgreens. Audit22 actual RED
retained, originalexit101 reconstructed and disclosed. Deferred-submit failure
is intentional bounded-entry handshake expectation, not orchestration timeout;
its later tuple-countassert didnotexecute. Historical393 two-helper proof is
separately archived, never current. Incorrect queued-shutdown assumption and
ENOSPC compile remain non-passes; corrected actualadapter tests show stopped-
executor submission, not detachedloss inside synchronous rejectedsubmission.
Actual sharedoffload single source variant failedcentral1+adapters8/restored9;
reconstructed inlineadapter variants failed8/restored8.

Pre-R1 measurements: 66000percall/flow rows plus5250batchwall rows, current/
inline/restored phases, allscenario med/p95/p99. These are NOT rerun exact616
measurements. First-poll percall scope excludes earlier scheduling; batchwall
starts BEFORE four spawned writers through ALLcompletion, captured BEFORE CSV
writes; N250 for each of7fixtures/phase. Legal declared Open/Closed states,
zero spawns, encodedbody equality, debugLinux/tmpfs, 2runtime/8blocking workers;
scriptedzero-network OAuth4 independentlanes. No production/network/speedup
claim. Restored oldmedians: emptywrite75.18usvs27.91inline;2048openwrite8.881msvs
7.684; four scheduled percall24.738msvs7.851; fullbatch35.338msvs16.245(N250);
OAuth2.466msvs0.874. Single slot serializes cloning/validation/encoding/I/O/cleanup;
normal pipelineunchanged but newcatch latency was NOT independently measured.

Gate checkpoints: original209 isolated fullSDK984unit/401app/212domain/65infra/
38docs and actual sixpkg suite PASS; latera8 hadonly cfg(test).ok removal and
artifactformat, all10hostedjobsPASSrun37670126367 on mergeefb855fe ofa8intod55,
all14actualcentralPASS onLinux/Mac/Windows. Seven currenta8 localcommands exit0;
local script348tests/346pass/0fail/2skip but process exit unavailable, not called
completedlocalcommand; actualhostedfrontendSUCCESS supplies that commandreturn.
Current616 focusedgate manifest/selfreview are /tmp/627-r1-final-handoff/gates/manifest.json and /tmp/627-r1-final-handoff/627-r1-implementer-review.md. All11 finite actualexit0, ownedrunner actualexit0; SDK21/records20/audit7, central15, exactsixpkg filteredcentral15namepresence, alltargetClippy/SDKrustdoc/architecture/fmt/frontendformat. Rootverifiedall30sourcehashes and25artifacthashes. No fullolder-suites relabeling. Public record updated commit33894ed0 beforethishandoff, including616 archive/selfreview.
Currenthosted run37675120022 active; root verifies fullsuites, all15 names and
required checks beforemerge. Never relabel older results as exact616 execution.

Canonical maintained public gate record (pendingstubdeleted):
https://github.com/nessalabs/nessa-agent/tree/codex/622-verification-evidence/verification/bounded-persistence/evidence/review-and-final-gates
It names checkpoints and observed/missing results rather than claiming pending
records are completedapproval. Root publishes current616 manifests beforehandoff.

Scopes: contracts reviewer authority/API/features/module organization/docs and
state/effect/typed outcome agreement; adversarial reviewer concrete neighboring
interleavings/currentfaultprobes/real effects/ownership plus measurement and
provenance. Both apply all relevant standards17gates/nine dimensions/agreement.
Source read-only, no Cargo without root allocation. Challenge concrete inputs/
interleavings, disclose personallyexecuted vs inspected evidence; a source/log
review is not personallyexecutedCargo. Ask root for specific reproducible tool
allocation ifneeded; don'teditfrozencheckout. Reports exactbase/head, findings
severity/location/trigger/expectedactual, enforcers/relationships, exclusions,
and approval only if no priorityfindings remain. Write /tmp/627-r2-contracts-report.md
and /tmp/627-r2-adversarial-report.md; send findings promptly.

Exact public616 gate record/archive: https://github.com/nessalabs/nessa-agent/tree/33894ed0/verification/bounded-persistence/evidence/review-and-final-gates . Currentmanualbinary OSmanifest /tmp/627-r1-final-handoff/gates/manual-os-manifest.json; source/log/artifact hashes retained. Mutations andactualLinuxprobes alsoon616 candidatebranch verification/bounded-persistence/evidence/r1-submission. Normal Cargo ownerroot; reviewersread-only, requestallocationforspecificsuspectedrepro.
