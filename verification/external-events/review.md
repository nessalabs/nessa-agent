# Independent review record (#1088)

Base: `0b8019cf45f99b285f10c41199875d474492c6d1`.
Final source checkpoint: `0c320bd82744247a771773562510d0e5c0b8b479`.
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
