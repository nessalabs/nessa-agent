# SDK ownership publication evidence — work in progress

Issue #628; draft PR #650. This folder preserves historical source-specific evidence, not a completed merge gate. Final corrected-source gates and independent review are pending.

## Historical source identities

| Proof | Exact source | Valid runtime failures |
| --- | --- | --- |
| Original publication rules | `7993434c6ac27b7e2dc9c9da8e89adfffb0ced04` (`codex/628-proof-publication-799`) | 27 |
| Factory gate and transfer | `cddefd5d00ffc01588b3688a135f4d6a3e35b5c4` (`codex/628-proof-factory-cddef`) | 6 |
| Restored history precision | `141755dde6fc159031047ae23f20c1f14dc6e2fd` (`codex/628-proof-history-141`) | 2 |
| Atomic uncertain-root seal | Corrected dirty source on parent `bb24a364`, committed as `1bf09a5ef83f2a4183f3dfbc05e057d5e0138a31`; manifest records restored source hashes | 2 |

Invalid compilation attempts are retained and excluded. The 37 failures above are historical runtime proof, not a claim that every probe ran on the final candidate. Before/after rebase SDK byte equality is recorded in `628-restack-evidence.json`.

Preserved gate directories identify their own source. Completed isolated SDK and six-package tests at `1bf09a5e` passed, but CI run `37622329149` failed the required domain coverage gate: graph.rs has three uncovered regions and one uncovered line. Its excerpt is retained. This candidate is not approved.

## Structural self-review correction underway

One admitted-spawn typed-error boundary will revoke graph/shared-gate runnable authority before fallible fallback publication or unused capacity return. Only the child created by this invocation belongs to that boundary; lookup/conflict/pre-admission failures cannot close another transaction's child. Actual cleanup owners, receipts and the first close cause remain factual. No physical absence or completed closure is inferred from error or Ended progress.

Closing vacant cleanup binding remains valid until actual Released, absence or Closed evidence refuses it. The legacy Open+Ended{Reserved} guard remains narrow. Historical factory completed-startup refusal evidence must be read against its original source; the current runtime graph-close correction changes that case and needs new regression evidence.

Panic supervision #625, mandatory cleanup publication debt #646, failed-startup parent settlement #649, and blocking adapters #627 remain separate. Final exact-source gates, supported-platform CI, both independent reviews and dispositions will be added before merge.

## Preserved final 1bf CI outcome

Run37622329149 is completed FAILED: Linux and Windows passed; macOS failed an existing desktop-stop ordering assertion (expected provider openings2, observed1), and domain coverage failed as above. Required-check aggregate failed. The metadata and exact failure excerpt are retained. Issue651 tracks the independently reachable reference-count-versus-actual-FIFO-admission gap; exact historical scheduling cause remains unproven. Its small verification correction is being prepared in an isolated worktree for inclusion in PR650 before final gates and fresh cumulative R1 review. No review round has started.

## Typed-error revocation checkpoint

`4db970d5e38b9a5afa118bee14539729dc2a3d2b` is preserved on `codex/628-proof-error-revocation-4db`. Twelve additional one-rule mutations compile and fail actual runtime assertions; the manifest records dirty corrected bytes over parent1bf and verifies restored hashes against committed4db. The retained historical total is49. These probes cover early graph/gate revocation, fallback await ordering, factory/submission failure, private rejected admission, admitted-only scope, preserving original errors, no duplicate first-close evidence on joins, legacy Open+Ended binding precision, and unknown sealed-progress query refusal. Restored focused application80/domain37/join tests pass. Exact-source domain coverage and the cumulative candidate are still pending.


## Assembled candidate e00: still pending final gates/reviews

SDK checkpoint `36e0b7ea17d8e59d58eb18f35f83c75667c57aeb` adds two reachable absence regressions: stale close operation cannot mutate physical facts; a matching retained token cannot acknowledge against refused restored history. The two actual runtime mutation failures bring historical SDK proofs to **51**. Their dirty-source parents, clean checkpoints, command output and restoration hashes are retained in `probes/absence-ed2` and `probes/acknowledgement-36e0`.

`coverage-36e0-preserved` retains the failed full SDK coverage at `4db`, failed/intermediate targeted reports, invalid CLI invocations that ran no tests, and the final supported preserved-profile report (100% lines/functions/regions). The source/profile manifest verifies all 268 SDK source files byte-identical through `36e0`, all 135 prior raw profiles preserved, and 34 new restored-source profiles. This is a full-suite-plus-targeted aggregate, not a fresh full-suite execution at `36e0`. The large raw LLVM JSON stays in the workspace; its byte count, digest and aggregate totals are checked into the summary. Mutants ran on a separate noninstrumented target.

`desktop-stop-d25` retains the independently reachable unpolled-future versus FIFO counterexample, 12 passing desktop-stop tests, the actual unconstrained-polling revert failure, restored pass, Clippy and self-review at `d25cfa7c68c3bec1886e65acfec7dd742679cfef`. The historical macOS scheduling cause remains unknown; this corrects the independently demonstrated fixture premise.

Cumulative head `e00ca7c286477e393b8edf8aa231e2124c6870bf` on base `f1682e8a763646c6c3e73dee73c1c47987e2cbae` contains exact d25 desktop source plus two SDK Rustdoc lines naming actual dispatch refusal errors. Assembly equality/delta is retained. Historical local coverage is therefore attributed to `36e0`, not claimed byte-identical to final source. Required CI run 37630379777 reruns full domain coverage at actual e00 with unchanged filters and 100% thresholds. Final normal isolated SDK and cumulative six-package gates are running; independent R1 has not started. These artifacts are not merge approval.


## Corrected frozen head 06dee: local gates passed, R1/CI pending

Root self-review found new fully qualified type paths and function-local test imports that conflicted with the repository import convention. Clean `06dee44de04faa5d172bcca8fa79eec919bb118d` applies only the bounded four-file import/type-spelling correction over e00. The actual patch and non-import token comparison retain the concrete-type/rustfmt-normalization proof; all-target Clippy validates resolution. No new executable rule or runtime mutation was added for this import-only delta.

`gates-e00-preserved` contains PASS full isolated SDK (library963/13 ignored; application400/4 ignored; domain212; infrastructure65/2 ignored; documentation38) and the exact CI six-package full tests/doctests (all pass). Root paused only its between-command controller while the active combined child finished against unchanged e00. Actual completed child OS status0 was captured, import changes were made after all Cargo children stopped, and controller resumed/reaped then intentionally stopped at its changed-head guard before launching further checks. That controller exit1 is not a suite failure. Full combined tests took568.23s including compilation; isolated full SDK took247.41s.

`gates-06dee-final` contains corrected-head PASS formatting, isolated application400/domain212, focused subagents library tests, combined six-package all-target Clippy with warnings denied, SDK docs, architecture tests and architecture check. Heavy unchanged infrastructure runtime was not repeated locally solely for moving imports. Required CI run37632187658 executes full supported-platform suites and full isolated domain coverage at actual06dee before any merge. Pre-import e00 run37630379777 completed domain coverage successfully then was superseded/cancelled; its partial job statuses are retained, never counted as current full approval. Two fresh cumulative R1 Sol medium reviews are running; no approval claimed yet.


Current runtime correction: `3e65d96e44e27c502a6225ec16e554fe18c4a152`. R4 found off-context queued root delivery Drop; actual red and scheduling revert are preserved under executor-probes. The original runtime handle now travels with the delivery obligation. Focused restored checks pass; whole final gates and exact-head CI remain in progress. The cumulative review history is in bounded-review-history.json; the next review is R5, without a budget reset. Earlier clean R4 reports were amended and are not approvals.

Cumulative R5 is now complete: both fresh independent whole-PR reviews report no remaining findings at any priority; complete reports and a considered/dismissed wording concern are retained. Root exact-head full isolated SDK passes. Combined root gates, current-head platform/domain coverage CI, and external review remain pending; no merge approval yet.

Exact3e65 final gates ALL PASS; complete raw logs/command and timing manifest in gates-3e65-final. Full isolated SDK plus exact CI six-package test/doctest selection and all-target Clippy, format, SDK docs, architecture tests/check all completed. Required current-head platform/domain coverage CI and external review remain pending; no merge approval yet.

IMPORTANT latest disposition: same-head R5 reports were amended after CodeRabbit identified a real accepted-binding-wins-absence race. One MAJOR/P1 remains, so earlier clean R5 conclusions are superseded. PR650 returned to draft per bounded5 rule. Proposed minimal correction is recorded as NOT APPLIED. Explicit user exception requested before source changes/follow-up review. Exact3e passinggates do not override finding.
