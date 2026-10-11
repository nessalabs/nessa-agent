# Browser measurement readiness corrections

Checkout: `/Users/nessa/.codex/worktrees/stability-audit/nessa-agent`.
Branch: `codex/691-unblocked-browser-readiness`.
Final clean commit: `4a3c9a900697d02bfd83e90237c07ae11ed140b0`; preceding readiness commit `7ca52f39c6b779197fe933fb17f24fdc8b339962`.
Base: `9c1c672f2f20e39f63ff9745a32d6467259d328d`.
Four-file delta: three owning verification scripts and the canonical sampling-order table. The first correction is 11 insertions/3 deletions; the following P2 closure is a two-file 5-insertion/3-deletion delta. Production frontend, gateway, native host, client packages, selectors, fixtures, seeds, policies, assertions and budgets are unchanged. `source-manifest.json` records file SHA-256 and this regional comparison.

## Actual mechanisms

1. Command order observed the overview shell before its deferred list was published. All eight original observations had `data-overview-listed` absent and zero rows; the existing publication condition then observed six rows about 260–290 ms later. All original card, row, peek, bidi and direction/geometry assertions held. The script now consumes existing `overviewListed` after each shell open.
2. The compact card's actual DOM was visible at opacity 1, 280×44 px, while the latest recorder sample was 14 ms older and had no ghost. The next two rAF samples held that same card. The script now waits for the current recording to publish a ghost, using existing `until`, before reading its unchanged measurements. Recorder reset and generation retirement remain owned by `recordShapeFrames`.
3. At actual innerWidth 820 px the original resize observation still had an open rail; two existing page frames later, at the same width, it had folded. The same occurred at 620 px. `useWindowWidth` subscribes to resize through `useSyncExternalStore`; the script now crosses existing `frames(page)` before existing settlement and its next rail predicate. The 20 px descent, 400 px floor and actual focus assertions remain unchanged.

## Canonical proof at clean readiness commit 7ca52f39c

| Check | Engines/layouts/default sizes | Final |
| --- | --- | --- |
| command order | Chromium + WebKit; columns + sidebar | 8/8, exit 0, 10.9 s |
| compact drag card | both engines/layouts; 1440×900 and 1000×700 | 8/8, exit 0, 37.8 s |
| rail fold/focus | both engines/layouts; 1440×900, 1000×700 and 760×700 | 12/12, exit 0, 49.6 s |

The final canonical runs are headless, no quick tier, with bundled Chromium and actual WebKit. Logs/JSON and exact source snapshots are in `clean-command`, `clean-drag`, `clean-rail`. Prettier, changed-file ESLint, `git diff --check` and all 164 desktop helper tests pass (no skips). No synthetic mirror test was added.

Individual source-removal probes were asserted to land, recorded with SHA-256, and restored byte-for-byte with fresh mtime:

- Command waits 2→0: original missing overview rows, 0/8, exit 1; restored 8/8.
- Compact publication wait 1→0: 6/8 held, two WebKit setup cases could not run, exit 2; restored 8/8. The race varies by row: the original focused headed WK/sidebar/1000 case could not run 3/3 times, while this all-combination removal failed WK/columns/1000 and WK/sidebar/1440. No result was discarded.
- Rail frame fence 1→0: headless 8/12 held, four Chromium setup cases could not run, exit 2; restored 12/12. Old headed canonical rail checks passed 6/6 on both main and the cut, so those are not claimed as the removal proof.

Actual current-main `88f34e4d` also failed canonical command order 0/8 and could not run the focused compact-card case. All first failures and diagnostic-only copies are preserved under the sibling `focused` directory. Diagnostic copies logged original observations before advancing publication; they are not substituted for canonical results.

## Whole-run and performance limits

The original complete functional run on clean source `9c1c672f` is preserved: 978/990 held, exit 1, 3114.7 s. Twenty of 23 groups held; four setup cases could not run and eight command-order rows failed. The corrected clean-commit results above do not retroactively relabel that whole run. A final assembled-cut whole run remains coordinator-owned and pending.

Separate retained-app dev coverage passed 4/4 (7.9 s). The production delivery check passed 14/14; three fresh-page runs per engine/layout measured active max 42.7/39.1 ms (Chromium columns/sidebar, 4×) and 35/33 ms (WebKit, unthrottled); maximum Chromium delivery frame 34.8 ms, zero frames over 50.

The unchanged full production performance gate at 9c1 remains failed 33/35: overview-answer columns max/median 76.8/73.3 ms and sidebar 72.7/65.5 ms, three over-50 frames each. Actual current-main five-run baseline also failed every overview-answer run, with max/median 87.9/76.5 and 75.6/73.8 ms. The 50 ms threshold is not waived, and no attribution to deferred #710 or product fix is claimed. No performance rerun was done merely to seek a pass.

## Self-review and handoff

Reviewed the complete diff against current CODING_STANDARDS, information ownership, agreement across sampler/DOM/publication, bounded wait behavior, gate 14 environment, gate 17 numbers, original assertions and repository organization. The published list, owned recording and existing frame fence retain their original owners. No new paths, dependencies, producer, marker, test switch or runtime policy were introduced; no module map move is required. No self-review findings remain. Fresh independent round 1 found one P2: ignored boolean result of overviewListed. That finding is closed in 4a3c9a900; different reviewer round 2 is coordinator-owned and pending.

Native WKWebView, live providers, operating-system file pickers and Linux install flows were not exercised. All owned browser, Vite, gateway, probe and caffeinate processes ended after final clean-commit runs; the checkout is clean. Unrelated orphan Chrome/Vite processes were left untouched. Raw ACP capability-bearing traces remain private and must not be published.

## Independent P2 closure at 4a3c9a900

Both overview publication callers now reject a false wait result through existing CannotRun, explicitly reporting that the complete list was not published. The canonical sampling table was updated before the guards. No helper, producer, fixture, selector, row/bidi/geometry assertion or other ignored wait changed.

The negative witness injects an observation failure only: document.querySelector for the existing published-marker selector returns unavailable. Real producer attributes and target rows remain intact. At both timeouts the actual producer-marker count is 1 and both legitimate command/direction target counts are 1. With guards the actual canonical-derived consumers return CouldNotRun, 0/2, exit 2. Removing only the two boolean guards (publication waits retained), with the same injected observation failure, makes both semantic checks pass; the unavailable-publication witness then fails exit 1 for this fail-open result. This injected boundary behavior is not claimed as observed product behavior.

The source removal was asserted 2→0, guarded source restored byte-for-byte with fresh mtime (guard-restoration.json), and healthy canonical coverage again passed 8/8 in 10.8 seconds. The final clean 4a3c9a900 command run is recorded separately as clean-guarded-command. Drag and rail executable bytes are identical to their clean 7ca52f39c successful runs.
