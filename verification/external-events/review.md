# Independent review record (#1088)

Base: `0b8019cf45f99b285f10c41199875d474492c6d1`.
Current source checkpoint: `7763614c1998cc428a9b6043cbe1ab10cebe6ec4`.
The earlier clean review below covered `0c320bd8…`; the CI follow-up is separate.
Draft: [#1092](https://github.com/nessalabs/nessa-agent/pull/1092).
Each round used a fresh reviewer under `AGENTS.md` / `CODING_STANDARDS.md`.
CodeRabbit explicitly declined automatic review because this PR is draft. Its
non-run is not an approval; these fresh local reviews provide the replacement
required by the standards.

Source edits belonged to the implementer. Reviewers read the actual source and
explicitly identified dirty state/evidence files; no reviewer changed shared code.

## Round 1

The reviewer independently ran the original 11 feature tests. Findings were
retained spare capacity in otherwise bounded constructors, new-domain placement,
public API documentation and overly broad documentation claims. The correction
uses `Box<str>` / `Box<[u8]>`, adds compact-input regressions, moves the identity
under `domain/value_objects`, enforces scoped missing docs and states the actual
binding/clock/receipt boundaries. All findings are closed in the initial source
commit `d8b508b1783f733ae04ed111690a6a04d3bc2616`.

## Round 2

The reviewer independently ran all 13 tests and found a high-priority resource
ownership gap: a borrowed HTTP permit was dropped when the response future was
cancelled, although durable submit work could continue. Existing service guards
were not a total admission-count bound. The owner approved a design revisit;
the ordering table now identifies the slot owner before/after handoff. The fix
transfers an owned semaphore permit into a per-admission task through the command
result, without adding a queue. Three regressions check pre-handoff cancellation,
post-handoff cancellation with eight gated storage calls, and task failure.
The copied-source caller-loss mutant fails when the permit returns to the caller.
Closed in `9c99ac6ec09123a02671587a85f0ef1f8b92c216`.

## Round 3

The reviewer inspected the complete base-to-source diff at `9c99ac6e…`, dirty state
page/navigation and evidence. Scope included signature/input bounds, identity,
target/request binding, current Auth/Cedar, receipt conflicts/restoration,
cancellation ownership, failure uncertainty, execution/stop reuse, module layout,
docs and mutation probes. All 16 feature tests independently passed; the
cancellation-capacity test also passed 30 repetitions. No production finding.

One minor synchronization finding remained: the cancellation test observed only
`after-release` completion, then inferred all nine provider starts from successful
replay receipts. A valid interleaving can enqueue an original turn after that
observed turn, so the count could run before its provider starts. Passing
repetitions did not prove the required observation order.

The structural correction separates admission evidence from execution evidence:
each original replay's receipt is retained and its execution is observed through
the existing authoritative completed-status view before counting effects. No
production code or new product state changed. Closed in `0c320bd8…`. A fresh
follow-up round checks this correction under the bounded-review-loop rule.

## Follow-up review

A fresh reviewer checked source `0c320bd82744247a771773562510d0e5c0b8b479`
against the recorded base, including the dirty state/docs/evidence. **No findings
at any priority; the review stop rule is satisfied.** The reviewer verified that
each original replay's execution is observed complete before the final count,
and independently ran all 16 feature tests: exit zero, 5.27 seconds.

The review covered signature/input bounds, current Auth/Cedar,
identity/target/request binding, durable receipt conflicts and restoration,
owned-slot cancellation, failure uncertainty, execution/stop reuse, public/module
boundaries, documentation and mutation evidence. HTTP and authorization source
hashes match their probe records. The reviewer made no tree changes.

The reviewer inspected supplied full-suite/browser evidence and did not repeat
those broad checks. Cross-platform behavior, production provider registration,
external effects and TCP-reset handling remain outside the review's execution
scope. The preserved baseline cleanup failure remains a gate blocker; this clean
source review does not override it.

## Preserved gate blocker and limits

The full selected package suite retains the unchanged Codex process cleanup
`CleanupUnconfirmed` failure, also reproduced by the exact test on the clean base.
No assertions or unrelated cleanup implementation changed. See [checks](checks.json)
and [baseline evidence](baseline-cleanup.txt). Hosted exact-head CI and parent
qualification remain required; the PR stays draft and parent owns the merge.

No review established production mounting, provider registration, real external
effects, cross-platform behavior or TCP-reset handling. Deterministic HTTP
service-future cancellation is the tested boundary. Browser-rendered docs and the
optional router are distinct from a desktop UI feature; shared stop/fail/restart
stress coverage remains with the existing conversation owner.

## CI fixture follow-up

Hosted Ubuntu run 39925277268 exposed twelve integration fixture failures at
`Fixture::new` line 246, `PermissionDenied(UnsafeFile)`. The temporary container
was created by TempDir with ambient permissions. Local umask 077 had hidden the
incorrect private-root assumption; the unmodified binary reproduces all twelve
failures under umask 022 as UID 1000. This was a new regression, not the process
cleanup baseline failure.

The scoped correction at `ebc5212723366e707256eed51a051d6c61bd3b5b` creates a
`private` child using `nessa_local_storage::create_directory`; no production
storage check or permissions are relaxed. The regression verifies the new storage
through the safe API, preserves the container's original mode, and executes a real
signed HTTP event through completion. Both 022 and 077 focused runs pass 17 tests.
A fresh independent reviewer checked source `ebc5212723366e707256eed51a051d6c61bd3b5b`
and the dirty docs/evidence. **No findings at any priority; review stop rule
satisfied.** Independent UID 1000 runs passed all 17 feature tests under umask 022
(5.50 s) and 077 (5.47 s); `git diff --check` passed. The review covered safe
ownership validation, Unix/Windows creation paths, reopen persistence, regression
portability and evidence claims. No files were changed by the reviewer.

The reviewer inspected supplied broad-suite, Clippy and browser evidence without
repeating them. The old candidate's browser pass does not qualify a later head;
the exact stress failure remains open, and local CleanupUnconfirmed remains
separately preserved.

The separate stress upgrade candidate-baseline ordering investigation does not
clear the failed browser stress gate. Existing helper suites and the client probe
were reviewed independently; their precise limits are in the hosted CI record and
verification README. No unrelated frontend/settings implementation changed.

## Windows cancellation-test follow-up

Run 39930446153 at `a1994569…` had one Windows test timeout at the post-release
ninth durable admission; every other check passed. The earlier browser, stress
42/42 and chaos 2/2 passes remain historical candidate evidence. The source log
does not prove a production permit leak or the exact cause of the slow schedule.

Source `7763614c1998cc428a9b6043cbe1ab10cebe6ec4` splits the test's capacity and
persistence obligations. Eight cancelled response futures still retain slots and
refuse a ninth before store entry. Controlled pre-storage failures settle that
case; observed failure/entry counts prove recovery without provider execution.
A separate successful caller-loss case observes actual terminal persistence
through the fixture port, waits execution complete and checks receipt replay and
one provider execution. The fixture observer notifies only after its underlying
terminal save succeeds. No production owner or timeout changed.

A fresh independent reviewer checked source `7763614c1998cc428a9b6043cbe1ab10cebe6ec4`
against published `a1994569…`, including dirty docs/evidence. **No findings at any
priority; review stop rule satisfied.** All 18 focused tests independently passed
under umask 022 in 5.27 seconds; `git diff --check` passed and production probe
hashes matched. No files changed during the review.

The review explicitly assessed invariant preservation: the prior successful
post-release calls did not measure capacity while successful writes stayed
pending. Their durability/replay obligation remains in the separate actual-store
case. Outcome-independent production permit scope is unchanged; the split does
not mask a demonstrated product defect. Reviewed gate/counter order, cancellation
joins, failure recovery, observer truth, replay, adjacent failed acknowledgements,
SDK ownership and documentation claims. Broad/browser checks were inspected as
supplied evidence, not independently repeated.

Windows itself was not executed locally. The exact original Windows timing cause
remains uncertain, and new-head hosted qualification is required. Historical
browser/stress/chaos passes and the separate cleanup failure remain preserved.
