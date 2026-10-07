# PR650 R6 independent contracts/API/docs review

**Recommendation: approve the scoped cumulative code change. No open blocker, major, minor, or nit finding.** This is current-head local review approval, not merge authorization or a claim that remote/platform/coverage gates passed.

Reviewed checkout: `/workspace/nessa-agent-owned-publication`.
Base: `f1682e8a763646c6c3e73dee73c1c47987e2cbae`.
Head: `b78bf641825d77c8be2e88f9ac76b1f14ed2a8dc`.
Prior R5 head: `3e65d96e44e27c502a6225ec16e554fe18c4a152`.
Actual RED checkpoint: `bb195df38dc9191bb9f73605eace01eab9832b24`.

I verified HEAD and empty `git status --short`, reviewed the whole 19-path cumulative diff against the stated base, and inspected the additional three-path correction against R5. I read AGENTS.md and CODING_STANDARDS.md, both required briefs, the prior contracts and adversarial reports including their amendments, affected module maps, canonical ADR329 table/diagrams/enforcers, and relevant architecture boundaries. This is the explicitly authorized narrow exception after five rounds; it does not restart the round budget or overwrite prior findings/history. No Cargo invocation, build, mutation probe, source edit, commit, or GitHub mutation was performed by this reviewer. The only written artifact is this report.

## Finding dispositions

### R5 MAJOR/P1 binding-winner drain abandonment — fixed

The confirmed R5 trigger was a legal public `bind_resources` accepted between `drive_drain`'s speculative owner/bound inspection and `settle_never_bound`'s admission-scoped recheck. Previously the helper safely refused false absence but converted that ordinary loser outcome to Incomplete; the enclosing `?` finished the original owned drain before claiming the accepted cleanup owner. Binding's notification could not revive that finished task.

Current `root.rs:124–129` returns Ok when the scoped bound fact wins, without taking an absence token or changing physical/evidence state. `coordinator.rs:1451–1474` marks the attempted processing and runs the existing loop again. That pass observes and claims the actual retained owner. `absence_attempted` remains true, so this is a bounded reinspection rather than another speculative absence attempt or retry controller. The graph remains lifecycle authority; the admission scope remains transfer/absence authority; the existing resource/claimed maps remain effect ownership. Actual audit/store/domain failures still propagate as typed failures.

The two new library tests call public open_root, never receive its returned ID, drop its queued unclaimed delivery, and hold the original owned reconciliation immediately after the outer empty/unbound observation. Public binding accepts a real owner behind Closing. The original drain is observed through its existing watch result with no end_lifetime call; assertions require Ok, one close, OwnerDisposed/Runtime, durable Closed/Released/Acknowledged, and no absence claim. The same gate-only handoff requires sealed Closing/Incomplete, no settlement and no absence claim. These assertions distinguish resource ownership from possible transfer and protect the negative boundary.

Inspected original RED and deliberate reversion logs: both compiled, ran two selected tests, passed the gate-only control, and failed the binding case at the original-drain result assertion, actual Err(Incomplete), required Ok. Inspected the dirty mutation patch: only the corrected skip branch is reverted to Err(Incomplete) relative to the corrected root source; surrounding coordinator rename/ADR diagram delta is checkpoint context. Restored-before/after digest files are identical. The exact-head isolated SDK and six-package logs both show these two cases passing. Disposition: fixed with meaningful runtime evidence; not merely an added timeout or a caller retry.

### Accepted absence audit / rejected terminal write / fresh restoration — deferred, still real

I re-traced `root.rs:130–145`, commit_snapshot, end_lifetime/drain caching, conservative restore, public transfers, and the base drain. An accepted absence audit can make the live graph Closed before its final snapshot write rejects. The returned failure is Store(Rejected), while the durable root remains Closing without the vanished live absence proof. Fresh restoration marks identities possibly bound and cannot truthfully manufacture absence; subsequent close can remain Incomplete and same-session root creation remains blocked. A live retry can republish the existing Closed graph before restart, which is a different case.

I inspected `/tmp/625646649-red/recovery-README.md`, the test-only recovery patch's described public seam, actual recovery RED assertion and positive controls. The recorded reproduction runs one test and fails after compilation at `observed never-bound absence must survive rejected terminal publication`, Err(Incomplete) versus Ok. Both conservative-restoration and previously-bound controls pass. Follow-on reopen/no-replay assertions are unexecuted while RED. These are historical next-slice evidence, not b78 execution of a repaired recovery path.

The R5 reclassification is retained: this operational gap does not violate an explicit current promise of automatic restart repair. Canonical row19 covers live retry after absence audit rejection, not accepted audit followed by failed final storage. Restored absence is explicitly refused; the base resource-free root already cannot settle. README/ADR preserve #625/#646/#649 boundaries. The correction adds no stronger durable-recovery claim and does not repair this gap. This disposition is not approval of the future repair design and does not excuse losing a real accepted live owner, fabricating acknowledgment, or returning fake durable success. Those new-regression conditions were checked separately above.

### Earlier rounds

Prior blanket stage/capacity documentation claims, table rendering defect, and off-context queued-ticket Drop defect retain their prior fixed dispositions. The current docs distinguish initial publication from later physical ownership/receipt retention, the canonical source contains contiguous rows1–38 without the old blank table separators, and DeliveryTicket carries the originating live Handle. The off-context regression and compiled free-spawn mutation log were inspected; runtime shutdown is explicitly outside that guarantee. The previously adjudicated phrase “The library’s” at the row14 enforcer remains contextual SDK ownership wording: the enclosing paragraph correctly names its public integration file, and the exact test exists. No new contradictory test-target claim was found.

## Required agreement across fields and layers

**Identity/authority:** OwnershipGraph owns lifetime/session/parent/request/operation legality; publication owns only eligibility and captured target/generation tokens. Clone shares the same coordinator Arc, rather than creating another graph. Public reads do not confer binding, descendant spawn or gate authority. transfer_refusal derives eligible ancestor closure and actual graph facts; refusal returns the exact supplied Arc. Domain targeted discard preserves neighboring graph state without a restore rollback. Domain absence tokens carry root/close/evidence correlation and independently recheck receiving-graph dispatch authority.

**Admission/delivery/physical ownership:** open_root's owned transaction begins when polled and remains owned until synchronous ticket claim on Ready. Sender success is not delivery. Captured Handle schedules queued/drop reconciliation on the original live runtime. Factory inflight owns its transfer slot and receives the remembered gate with the same scope/seal; real-Agent tests compare Arc identity and reject actual attachment after seal. Public Closing cleanup binding is distinct from runnable admission. Bound/gate handoff and absence claims share the admission scope; a report or Ended milestone cannot establish resource absence.

**Cause/stage/result/audit:** graph close joins preserve the first cause and initiator. Admitted typed-error revocation seals graph and actual child/descendant gates before fallback awaits/slot return; conflicts and pre-admission errors cannot revoke another child. Projection retains physical preparation and acknowledged provider receipts in nonrunnable shapes without upgrading pending affirmative permission. Actual physical release, audit acknowledgment and successful storage remain separate facts. root absence acknowledgment consumes the actual audit result; rejection stores Failed/Closing and permits the defined live evidence retry. Existing extra cleanup evidence debt is identified rather than mislabeled as fixed.

**Persistence/restoration/consumption:** scope-protected projection and revision are copied together; actual writes serialize and only acknowledged newer revisions suppress older copies. Unrelated accepted commits exclude private roots/reservations/reports. Suppressed reports and current safety facts persist despite rejected safety audit. Restored history preserves request/session/cause/receipt/physical relationships; contradicted history retains readable evidence and sealed inspection gates while refusing transfer/dispatch. Active root lookup excludes child/private/Closed history. Tests include held audit before/after unrelated saves, late acceptance/rejection after sealing, write-then-uncertain, valid retained history, stale token/operation, refused receiving graph, saved receipts and public cleanup rebinding.

**Desktop service boundary:** the private submission_future extraction preserves the same submission body, independent spawned ownership, asked flag and typed refusal mapping. The fixture polls that same real future unconstrained while close holds mode_changes, asserts cooperative-budget exhaustion and retained lock reference, then spawns the same pinned future and checks the receipt execution ID. The independent unpolled-lock counterexample establishes why reference count alone is not FIFO admission. Canonical conversation-admission wording matches that actual boundary. No production desktop-stop behavior redesign is claimed and the historical macOS failure's precise cause remains unknown.

## Review dimensions

| Required dimension | Inspection and limits |
| --- | --- |
| Authority and identity | Traced one graph, shared Clone/gates, publication tokens, public transfer refusal, ancestor closure, targeted discard and root lookup. No second lifecycle authority found. |
| Domain invariants | Read new graph helpers and pure regression assertions for legal root deletion, child/stale/Closed absence rejection, refused receiving graph and sealed receipt retention. Rejection preserves prior authoritative history. |
| Admission and concurrency | Traced root delivery/drop, late acknowledgment, factory flight, immediate error revocation, simultaneous owner/absence winners, gate-only loser and existing drain claim/notify/reinspection. Deterministic fixtures exercise exact boundaries. |
| Failure and cleanup | Checked typed primary errors, actual owner return/retention, release before fallible ports, once-only close, joins and retry boundaries. Full constructor/poll/Drop panic supervision and blocking adapters remain excluded. |
| Observations and restoration | Checked same/stale publication generations, valid and contradicted history, receipt retention through close/reload, no factory replay, conservative restored ownership and the deferred lost absence proof. No new wire parser/provider observation protocol is introduced. |
| Audit | Checked target/close operation/cause/initiator through root reconciliation, held/rejected/uncertain audits, Released versus Acknowledged, and original typed-error priority. Additional cleanup audit/storage repair remains explicit #646/#625 work. |
| Representation boundaries | Checked new typed public refusal/owner payload, immutable domain token storage, substitution fixtures, Clone gate identity and Debug avoiding gate methods. No new JSON/OS-path/serialization boundary in the diff. |
| Resource bounds | New retained metadata is identity/history bookkeeping, distinct from live capacity; flight release and physical Released capacity behavior are checked. Existing snapshot/ancestor scans and intentionally retained history are not claimed as new bounded-memory/incremental-performance guarantees. No benchmark or allocator measurement performed. |
| Public surface and organization | Inspected every new/changed exported helper, token, failure/refusal variant, PrepareRequest field, binding/lookup API, Rustdoc, module exports/maps, README and canonical links. Test controls remain cfg(test), no public test configuration, compatibility alias or speculative lifecycle abstraction. |

## Seventeen gates

| Gate | Assessment |
| --- | --- |
| 1 typed failures | Typed PortFailure/OwnershipFailure/BindResourcesRefusal matches; skip is a normal competing-transfer outcome, real failures remain typed. |
| 2 names | Feature vocabulary and processed_any describe actual attempted processing; no new controller terminology. |
| 3 boundaries | Lifecycle legality remains domain, projection/root transactions application, effects injected. Desktop extraction remains private application code. |
| 4 scope | 19-path cumulative SDK change plus documented #651 fixture extraction; final correction is three paths and the confirmed race. |
| 5 failure tests | Read row fixtures across rejection/uncertainty/drop/store/owner refusal/stale/restored combinations and the new resource versus gate control. Exact affected suites in logs pass. |
| 6 applicable checks | All eight exact-head local gate statuses/logs verified below; reviewer did not execute them. Current required remote/platform/coverage checks remain coordinator-owned. |
| 7 honest degradation | Gate-only/unknown restored ownership stays Incomplete; physical release does not become fabricated absence/audit/storage success. Deferred recovery limitation remains documented. |
| 8 organization | Reviewed module ownership, maps, test paths and links; no moved facade or new parallel lifecycle feature. |
| 9 seams | Audit/store/factory/capacity remain injected; tests exercise real substitutes and typed failures. No new external read bypass. |
| 10 enforcers | Canonical38-row table names concrete integration/library/domain/real-Agent tests. Corrected row22 names both exact-boundary regressions; guarantee scope matches assertions. |
| 11 construction | Private correlated token fields, typed variants and constrained domain identities carry rules; no error-string classification or blacklist. |
| 12 stable identity | Metadata keyed by minted lifetime/request/report identity and captured generation, not mutable session attributes. Session lookup is read-only. |
| 13 one owner | Domain graph owns transitions; gate shares its scope/seal; publication derives permission projection. New skip delegates to existing drain rather than another recovery owner. Meaningful new and representative historical rule reversion evidence inspected. |
| 14 where/load | Verified local Linux environment and actual CI six-package selection/concurrency2 runner, isolated SDK, all-target Clippy and warning-denying rustdoc. macOS/Windows/current coverage not independently verified. |
| 15 order first | RED commit already contains refined canonical row22 and held-boundary tests; correction adds the matching diagram/reinspection. Existing canonical rows1–38 correlate to inspected source/fixtures. |
| 16 simplest honest | Reuses scope and one extra existing-loop pass; no new persisted state, outcome/controller or retry API. Gate-only case stays honestly unconfirmed. |
| 17 browser | Inapplicable: SDK lifecycle plus private testability extraction/desktop ordering fixture; no changed UI, production gateway, ACP or MCP behavior in this diff. No browser verification claimed. |

## Performed checks and verified evidence

Executed read-only git HEAD/status/diff/diff-check inspection and Python digest/table verification. `git diff --check base HEAD` returned0. Programmatic source-table count confirms exactly contiguous rows1–38. Inspected historical markdown-it14.3.1 proof and before/after ordered row list; no fresh renderer execution or hosted GitHub rendering check was performed.

Independently recomputed and matched all three source digests and all eleven log digests in `/tmp/650-binding-race-evidence/manifest.json`; checked identical restoration files. Read gate runner, eight statuses, actual success summaries and relevant named tests. The frozen exact-head evidence records Linux/offline Cargo, shared target, build jobs2, Tini supervision and six-package runtime concurrency2. Eight exits are0: fmt, SDK static docs, architecture tests (74), architecture checker, six-package all-target Clippy with -D warnings, isolated SDK full suite, six-package combined tests/doctests, and warning-denying SDK rustdoc. Isolated SDK counts are965 library passed/13 ignored,401 application/4 ignored,212 domain,65 infrastructure/2 ignored,38 doctests. Combined logs independently include the two new cases, the desktop ordering fixture and doctest results, with no failing result. A zero-byte fmt log is consistent with the recorded successful command; it is not separately treated as runtime lifecycle proof.

Read representative historical atomic-seal, off-context-executor and refused-history acknowledgment mutation manifests and actual assertion logs. Their recorded runtime failures support unchanged rules; I do not claim every earlier mutation was rerun on b78. Prior coverage attribution remains historical, not current-head coverage. No old-head external approval is counted as current approval.

Unverified: current remote CI, supported-platform execution, fresh coverage, arbitrary panic/runtime-shutdown/blocking-adapter behavior, production child composition and browser flows. These remain coordinator gates or stated separate work. This review approval is limited to the actual scoped cumulative contract and corrected live race, with the deferred RED retained as described above.
