# Verification

Scripts that drive real browsers against the running app and measure what a
person sees and does: frames, rects, where the caret is, what is painted under
the window's controls. The rule for when a UI change needs them lives in
[Browser verification for UI](../CODING_STANDARDS.md#browser-verification-for-ui);
this folder is where they live and how to run them. The
[`desktop-verification`](../.claude/skills/desktop-verification/SKILL.md) skill
says which to run for which change and how to report the result.

They are **not a CI gate** ([adding a check to CI](../CODING_STANDARDS.md#adding-a-check-to-ci)):
they need a browser, a server and minutes. Run them before handing off UI work,
when reviewing it, and after anything performance-sensitive.

## Layout

```
verification/
  README.md                 this file
  desktop/
    CHECKLIST.md            what we always test and reproduce, with the contract each item holds
    scripts/
      run-all.mjs           every check, summarised
      smoke.mjs             loads, sends, splits, Settings, overview; console errors
      perf-budget.mjs       the frame budget, production build, 4× CPU throttling
      safe-area.mjs         nothing painted under the window controls, per frame
      drag.mjs              pane drag: pointer path, zones, cancels, selection
      focus.mjs             where the caret lands after each pane and dialog change
      responsive.mjs        approval card, composer controls, column titles, Settings widths
      lib/
        selectors.mjs       every selector, key chord, storage key and known-harmless message
        cli.mjs             options, stderr diagnostics, JSON result, exit status
        server.mjs          reuse/start the dev server, or build + preview production
        browser.mjs         launch Chromium/WebKit, seed preferences, collect errors
        workspace.mjs       open panes, read pane rects and focus, lift a pane
        safe-area.mjs       the per-frame safe-area sampler
        perf.mjs            rAF gaps, Long Animation Frames, long tasks, throttling, calibration
        run.mjs             the main every check shares
```

When the UI moves, edit `lib/selectors.mjs` — nothing else should name a class,
a label, or a key. A script that cannot find what it needs stops with
"could not run" (exit 2) and names the selector, rather than guessing.

## Prerequisites

- `pnpm install` (brings the pinned `playwright` devDependency).
- Google Chrome installed (the default Chromium channel; it has Long Animation
  Frame timing). Or pass `--channel bundled` after `pnpm exec playwright install chromium`.
- WebKit for Playwright: `pnpm exec playwright install webkit`.
- Nothing else running on 127.0.0.1:1438 except, optionally, `pnpm desktop:dev`
  (a running dev server is reused; otherwise one is started and stopped).

## Running

```sh
pnpm verify:desktop                     # everything (functional checks on dev, perf on a production build)
pnpm verify:desktop --skip-perf         # everything but the budget
pnpm verify:desktop:smoke
pnpm verify:desktop:perf --runs 3 --only split-right,drag-drop
pnpm verify:desktop:safe-area --engine webkit --sizes 1000x700
pnpm verify:desktop:drag
pnpm verify:desktop:focus
pnpm verify:desktop:responsive --shots /tmp/desktop-shots
node verification/desktop/scripts/<check>.mjs --help
```

Every check takes `--url`, `--mode dev|prod`, `--engine chromium,webkit`,
`--layout columns,sidebar`, `--quick`, `--headed`, `--out <file>`,
`--shots <dir>` and `--verbose`; `--help` lists its own options.

**Two tiers.** While iterating, run the check that covers the change with
`--quick`: the first engine, layout and size of its defaults — unless named
(`--quick --engine webkit`) — so it answers in seconds (`safe-area --quick`:
12 scenarios, about 30s). It is not evidence for a hand-off. Once the change
holds, run the whole set once (`pnpm verify:desktop`, about 5 minutes without
perf): every engine, layout and size. `safe-area` runs its scenarios four at a
time (`--jobs`), each worker reusing one page per engine, layout and size, and
ends each step once its motion has, not after a fixed wait.

## Output

- **stdout**: one JSON document — `{ check, ok, target, results: [{ name, engine, layout, ok, failures, … }] }`
  — or written to `--out`.
- **stderr**: progress, `ok` / `FAIL` / `ERROR` per result, and tables.
- **exit**: `0` every assertion held; `1` an assertion failed; `2` could not
  run (a selector not found, a browser missing, the server did not start).

Per [machine-readable command output](../CODING_STANDARDS.md#machine-readable-command-output).
