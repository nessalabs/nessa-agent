# #712 current consumer correction

Checkout: `/Users/nessa/.codex/worktrees/unblocked-panel-list/nessa-agent`
Branch: `codex/712-unavailable-draft`
Base: `bd8ef0763b92a6f5ca6a7178d736b21a7a5ffe17`
Clean source head: `f803d375a1036d73016b38cd6822e8c9e2947237`

Only five source/doc/test files changed; list and SHA-256 values are in `handoff-source.json`. No paused issue implementation, history API, paging/render, attachment representation, ManagedSession or other mode controls imported. No public GitHub mutations or push.

## Behavior and ownership

The existing pure `declineReason` now consumes a bound conversation's defined typed failed-read fact immediately after disconnected-session refusal and before file decisions. It returns `view-unavailable`, retains the draft and asks the existing follow owner to retry. The existing App approval row consumes the same published fact in its disabled chain. This UI projection is not another authorization decision. No new decline union or predicate/guard owner was introduced.

Existing ADR231 #712 ordering rows were extended before the source rules. Public store witnesses deliver typed unavailable through the follower port, preserve text/files, remote view, target, selection, local turns and phase, and assert zero create/send/steer. Valid subsequent publication clears failure and allows genuine text+linked-file send and steer. Existing pure decision tests preserve unbound/no-view admission and prove session -> failed view -> attachment precedence.

## Checks on final source

- 877/877 conversation+panel tests, 74 files; final-scoped-tests.log. No skipped tests.
- TypeScript typecheck, changed ESLint and Prettier, git diff --check: pass (final-typecheck/lint/format.log).
- 164/164 existing desktop helper tests: pass (desktop-helpers.log).
- Own correct bd8 Rust-source gateway rebuilt with `cargo build -p nessa-server`: pass, 21.67s (gateway-build.log). This correction changes no Rust source; private binary SHA in manifest. No shared build artifacts used.
- Unchanged actual `mode-publication.mjs`, dev server, headed bundled Chromium+WebKit, no quick mode: final 3/3 held, exit 0 (browser-final.json/log). Both consumers publish Ask -> Auto -> Ask without transcript messages, source revisions [1,2,2,3]; typed unavailable preserves the visible draft, dispatch delta 0 and view-unavailable refusal; confirmed recovery restores Auto and enabled controls. Six screenshots are under final-shots; both unavailable screenshots visually inspected, showing disabled Tool approval / Ask and retained draft.

## Deliberate rule breaks

- Exact base missing-guard public witnesses: 3 failed / 61 passed; create/send=1/1 and create/steer=1/1; attachment refusal incorrectly precedes view failure (baseline-witness.log). An initial fixture mistake using a nonexistent scenario read method was corrected before this canonical baseline; it supplies no behavioral evidence.
- Source-asserted draft guard 1 -> 0 removal: affected public witnesses fail identically (revert-witness.log, exit 1), restored. Unchanged live two-engine browser also fails (browser-revert.json/log, exit 1). Its emitted exception is the later mode ACK after admitted draft input. The earliest violation is erroneous create/send admission and draft loss, proved by the public witnesses and parent's existing compact gateway diagnostic; the downstream ACK is not claimed as root cause.
- After draft fix alone, unchanged browser exposed a separate confirmed current-mode consumer omission: retained enabled approval row in both engines (browser-fixed.json). Draft/target/source/recovery behavior already held.
- Source-asserted removal of only the approval-row readError line, with draft guard retained: both unchanged browser consumers fail exactly `actual panel retained mode controls after an unavailable view` (browser-ui-revert.json/log, exit 1). All draft, source-revision and recovery positive counterparts still hold. Condition restored; final browser 3/3 held.
- Exact source snapshots and hashes for individual removals/restoration: browser-revert-source.txt, ui-revert-source.txt, browser-final-source.txt. Fixture/script bytes match base (asserted in final manifest); no assertions or diagnostic AST changes.

## Self-review and limits

Reviewed all five-file changes against current CODING_STANDARDS gates, agreement across fields/layers and organization. One existing decision owner; typed failure and bound identity; unavailable/refollow/recovered order; immutable draft/view; no changed dependency boundaries or paths, so existing module maps remain valid. Both touched source rules are independently load-bearing. No self-review findings remain. Fresh independent UI review and final combined suite are coordinator-owned and pending; previous regional Rust/API evidence is not replaced by this narrow consumer run. This is scoped browser verification, not a claim that full run-all/performance was rerun here.

All owned caffeinate, Vite, gateway and browser processes ended. Clean git tree. Browser ownership released. Gateway diagnostics remain private: do not publish raw ACP logs or capability-bearing diagnostic data.
