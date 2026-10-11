# Unblocked desktop review, ordinary round 1

Result: **zero findings at every priority in the owned review scope**. This is a fresh source/code review of the new main-based cut, not an approval inherited from the parked whole audit branch.

- Exact reviewed head: `4fea9814cb7ba5bc6debaff9cf0a01e1cbcc5b3b`.
- Intended base: `965797a9f2cb1a71923f32eee4197aa501669ebf`.
- Independent attached checkout: `/Users/nessa/.codex/worktrees/audit-sdk-client-lifetimes/nessa-agent`, detached and clean at handoff.
- Scope: #693 title/overview ResizeObserver commit staging and attribution; #730 workspace settlement frame lifetime; #722 Messages active/archived list pair, application port/adapters/UI and generated list capacity; the carried hover/overview/MCP verification readiness changes; new real-gateway mode-publication fixture/script and dev-only orchestration. Production SDK/native/OAuth/server correctness belongs to the other reviewers.

## Authority, identity and agreement

For #730, the mounted `useFocusFollowsPane` effect is the lifetime owner. All three store routes use its single `afterPaint`: pane settlement, window widget settlement, and both stages of return from overview. Cleanup retires before cancellation, including the active target search. Callback delivery after cancellation cannot begin child work. The existing `focusAfterPaint` remains the child target-search owner and keeps modal/person-focus arbitration. Checked the relationship between current store/scope, pane or widget destination, queued versus retired work, and current caret authority. The hook's public mounted tests exercise cleanup before first/between overview frames, late delivery, replacement, positive mounted stage counts and person/dialog behavior. An independent real-main source revert failed 9 of 29 focus tests; restoration passed 29/29. There is no layout-fixture setup widening in this new diff.

For #693, ResizeObserver callback owns measurement only; one pending frame per effect commits its latest widths/arrangement. Placement, overview width and cleanup cancel the owned pending frame. Measurements updated while a frame is pending replace its input, and unchanged final arrangement produces no redundant state update. Checked source and controlled observer/frame tests for initial size, threshold changes, multiple observations before paint and removing a title/unmount. The architecture/state map explains this ordering. Browser line attribution supplies actual engine/layout by default; an exception is still a failed console row, not a silently successful engine cell.

For #722, the history follow scope owns the two list registrations and refresh generation, while each gateway adapter token owns callbacks from its current subscription. The SDK subscription gate owns abort-before-answer physical close and next-open serialization. Reducers accept only the current follow generation and reject late one-shot answers. Each half retains its own listing/failure; explicit archived evidence excludes roster rows, and omission does not manufacture archive/delete evidence. Incomplete archived evidence retains known identities except explicit active contradictions; explicit current archived rows continue to win. These independent bounded reads are explicitly documented as non-atomic, so a temporary overlap is handled rather than falsely claimed impossible. Commands seed a new generation after their answer, preserve local archive/delete facts and the original UI cleanup owner. Stopped/retired errors do not publish diagnostics. Capacity two comes from `x-subscriptionLimits.listTargets` and is generated into Rust/TypeScript; the adapter does not copy that policy. Maps/ports/UI tests remain with the owning conversation feature.

Independent adversarial public-store probes passed: two stores using the same effects adapter retain separate follow owners; a Redux subscriber replacing a follow reentrantly during `historyFollowStarted` leaves only the replacement alive, including synchronous initial frames and synchronous callbacks during retired stop. The retired caller's later cleanup cannot stop the replacement. Scratch test content was restored byte-for-byte; no probe remains in the checkout. The temporary test edit changed that test file's mtime; no production or shared artifacts depend on this private checkout. The production focus revert preserved bytes and mtime explicitly.

## Verification code and representation

Traced the source relationships rather than accepting the old evidence labels. Hover waits for actual finite animations; overview measurements wait for the published full-list marker both initially and after reopening; the MCP message check reacquires its current app frame and establishes trusted pointer entry before its one click. Failure branches retain earlier failures and distinguish missing input from an Allow actually clicked. These change sampling, not budgets or product dispatch policy.

Both live-gateway fixtures import real production consumers, effects, stores and client subscriptions. The mode fixture's `refreshSource` obtains the currently followed transcript through `transcriptOf`, not a fabricated forced revision or one-shot read; the script checks gateway revision and semantic equality with no transcript output, actual panel tray mode, source publication, unchanged repetition, typed unavailable draft retention/refusal and real-gateway recovery. The injected unavailable case is at the application port and is described as such. Both new fixture runners are classified dev-only by the shared CLI owner, with helper regressions; production run-all skips are not claimed as passes. Their own clients/pages/gateway stacks have cleanup.

## Fresh checks in this independent checkout

- Scoped production tests: 72/72 across six files (focus, overview width, column header, history follow, gateway list follow, Messages UI). `/tmp/unblocked-desktop-review-unit.log`.
- Wider affected neighbors: 713/713 across 41 files (all conversation tests plus layouts, overview and quiet overview). `/tmp/unblocked-desktop-review-neighbors.log`. These overlap the scoped run; counts are not added into a fabricated total.
- Independent additional public-store probes: final history file 16/16, of which two are the new scratch probes. `/tmp/unblocked-desktop-independent-probes.log`.
- Verification helpers: 164/164. `/tmp/unblocked-desktop-review-helpers.log`.
- `pnpm typecheck`: PASS. `/tmp/unblocked-desktop-review-types.log`.
- Source-asserted main focus revert: 9 failed/20 passed, restored fixed source 29/29. `/tmp/unblocked-desktop-focus-main-revert.{json,log}` and `/tmp/unblocked-desktop-focus-restored.log`.
- `git diff --check`: PASS; final checkout clean. Hash inventory: `/tmp/nessa-unblocked-desktop-review-round1-source.json`.

All owned commands have ended. No browser, gateway, dev server, compiler, or power assertion was left running by this review.

## Scope exclusions and remaining merge evidence

The new diff imports none of the parked blocked-by-restructure feature fixes for #678/#680/#681/#682/#683/#686/#696: no client event/decoder/preset-validator or semantic history API porting was reviewed or undertaken here. The Messages history file is the existing UI catalogue projection, not the blocked semantic history endpoint. Existing main defects, including copied preset validation, were not silently repaired or approved.

This report excludes API #712/#726/#727 changes added after this head; SDK execution, native service persistence, Linux OS behavior, OAuth authority and server access/peer semantics; real native WKWebView; and the final assembled browser/scripted/performance gates. I inspected the existing author's source-qualified browser/revert evidence only as regional supporting evidence. I did not execute browsers or performance during this review. The coordinator still must run the exact final combined tree's required browser, scripted, performance and CI gates and publish their real results. A clean scoped code review is not a claim that gate 17 has passed. Later UI or verification source edits require a new applicable review; API-only additions may reuse the unchanged reviewed files with explicit source identity and assembled verification.
