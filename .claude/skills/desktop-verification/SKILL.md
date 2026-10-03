---
name: desktop-verification
description: Verify desktop window UI changes in real browsers (Chromium and WebKit) with the scripts under verification/desktop — run the checks that cover a change before handing it off, reproduce a reviewer's finding as a script, measure the frame budget in a production build, and report the numbers as evidence. Use when changing or reviewing anything a person sees or does in src/desktop (layout, motion, focus, drag, titlebar, responsive widths, Settings, the Agents overview), when a reviewer reports a visual or timing bug, or when asked about the performance budget.
---

# Desktop verification

The rule this skill serves is
[Browser verification for UI](../../../CODING_STANDARDS.md#browser-verification-for-ui);
read it first. The folder is described in
[`verification/README.md`](../../../verification/README.md), and the things we
always test are in
[`verification/desktop/CHECKLIST.md`](../../../verification/desktop/CHECKLIST.md).
This skill is how to use them; it restates neither.

## 1. Pick the checks that cover the change

| The change touches | Run |
| --- | --- |
| anything in `src/desktop/` (always, first) | `smoke.mjs` |
| titlebar, column heads, side columns, Settings chrome, the picture band, anything that slides | `safe-area.mjs` |
| drag and drop, `drop.ts`, `drag.ts`, pane headers | `drag.mjs` |
| keys, `focus.ts`, panes being added/closed/focused, dialogs, the overview | `focus.mjs` |
| widgets: `src/desktop/widgets/`, a widget's pane or the window, Escape for the widget in front, the edge peek's Escape | `widgets.mjs` |
| MCP Apps: `src/desktop/widgets/app/`, the sandbox proxy, `src-tauri/src/app_sandbox.rs` | `mcp-apps.mjs` |
| who asks for an approval: an app's review on the card or in the overview (`approval-request.tsx`, `gateway-views.ts`) | `app-review.mjs` |
| widths: approval card, composer (its thinking control too), column titles, Settings sidebar | `responsive.mjs --shots <dir>` |
| `index.html`'s load fallback, the panel's stage and window size | `load-fallback.mjs` |
| motion, FLIP, rendering, selectors, anything on the budget's list, or a perf claim | `perf-budget.mjs` (production build) |
| a UI branch before hand-off | `run-all.mjs` (all of the above) |

Then walk the CHECKLIST groups the change touches and do their manual items.
A behaviour the change adds or alters that no script covers is a gap: add the
check to the script that owns that area (or a new script sharing `lib/`), and
the item to CHECKLIST.md with its contract.

## 2. Run

```sh
pnpm desktop:dev                        # optional; a running dev server is reused
node verification/desktop/scripts/smoke.mjs
node verification/desktop/scripts/safe-area.mjs --only settings,edge-peek
node verification/desktop/scripts/perf-budget.mjs --runs 3 --only split-right,drag-drop
node verification/desktop/scripts/run-all.mjs --skip-perf --out /tmp/verify.json
```

`--help` on any script lists its checks and options. Useful ones:
`--engine chromium|webkit|chromium,webkit`, `--layout columns|sidebar`,
`--only <names>`, `--headed`, `--mode prod`, `--out`, `--shots`.

Iterate on the quick tier, then run the whole set once:

1. **While working:** the check that covers the change, with `--quick` (one
   engine, one layout, one size — add `--engine webkit` when the change is
   WebKit-sensitive) and `--only` for the scenarios it touches. Seconds, not
   minutes. Not evidence.
2. **Once it holds:** `run-all.mjs` on the final tree, every engine, layout
   and size, and `perf-budget.mjs` if the change is on the budget's list. This
   is what the report cites.

## 3. Read the output

- stderr: `ok` / `FAIL` / `ERROR` per result, with each failure on its own
  line; perf prints a table (max, median, frames over 50 ms, each run).
- stdout / `--out`: the JSON; each result keeps its detail (a focus trail,
  the frames that broke, LoAF attribution for slow frames).
- Exit codes are in [`verification/README.md` › Output](../../../verification/README.md#output).
  Exit `2` is not a pass: read the message; if the UI moved, update
  `lib/selectors.mjs`.
- perf: trust a row only after `calibration` held. Report max **and** median
  over several runs; attribute every over-budget frame from its LoAF entry
  (script, source position, forced layout) before proposing a fix. Numbers
  from `--mode dev` are not the budget's.

**Known-harmless noise** (already filtered or labelled; see CHECKLIST.md › Console errors):
the fresh-browser `/favicon.ico` 404; a dev-server reload because someone else's edit landed mid-run (re-run). A
single headless perf outlier: re-run `--headed` with more `--runs`.

## 4. WebKit

The app runs in WKWebView. Scripts default to both engines where the engines
differ (safe area, drag, focus, responsive, smoke); `perf-budget` is
Chromium-only. When a failure is WebKit-only:

1. Re-run that check alone: `--engine webkit --only <name> --headed`.
2. Decide whether it is Playwright's WebKit or the product: Playwright's WebKit
   differs from WKWebView in places (pointer events outside the viewport).
   Reproduce it in the app (`pnpm app`) before calling it a bug, and say which
   you did.
3. For an intermittent WebKit failure, re-run it a few times and report the
   count ("failed 1 of 3"), so the report says how often rather than whether.

## 5. Prove the check bites (revert probe)

A check that passes against the bug proves nothing. For a fix, or a new check:

1. Run the check on the fixed tree — it holds.
2. Revert the fix in the source (only the fix's hunk), and **show that the
   revert landed** (`git diff` the hunk, or print the changed lines). Touch the
   file so Vite reloads it; with `--mode prod` the build is fresh each run.
3. Run the same check — it must fail, with the failure the bug produces.
4. Restore the fix, confirm `git diff` is back to the intended change, and run
   the check once more — it holds.

This is [evidence and closure](../../../CODING_STANDARDS.md#evidence-and-closure)'s
revert rule applied to browser checks. A revert changes the tree others may be
building from: do it in a tree only you are editing, or agree it first.

## 6. Turn a reviewer's reproduction into a script

A reviewer's scratch script that found a real bug belongs under
`verification/desktop/scripts/`: port it onto `lib/` (`openPage`, `need`,
`attempt`, the samplers, the condition waits), move its selectors into
`lib/selectors.mjs`, make it assert numbers (rects, frame times, the active
element, zone sequences) rather than print them, and add its item to
CHECKLIST.md. Then do the revert probe above to show it catches the bug.

## 7. Report

Report against [evidence and closure](../../../CODING_STANDARDS.md#evidence-and-closure):

- the exact tree (`git rev-parse HEAD` plus "with uncommitted changes to …"),
  mode (dev/prod), engines, layouts, sizes, and the commands run;
- per check: held / failed / could not run, with the failing lines verbatim;
- perf: the table (max, median, over-50 per row, runs, throttle, calibration
  ratio), and attribution for any over-budget frame;
- for a fix: the revert probe — failing before, holding after;
- what was checked by hand (which CHECKLIST items), and what was not run and why;
- anything intermittent, with its count, and anything WebKit-only, with
  whether it was reproduced in the app.

Label a hypothesis as one. A green run of the wrong check, engine or build
is not evidence.
