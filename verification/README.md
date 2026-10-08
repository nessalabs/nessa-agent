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

A large-workspace measurement is specified in
[UI workspace load](../docs/design/ui-workspace-load.md). The seeded page is
opt-in (`workspace-load.mjs`). It is not part of `pnpm verify:desktop`.

## Layout

```
verification/
  README.md                 this file
  conversation/
    README.md               committed transcript fixture and publication ownership
    fixture.html            real ConversationControls and applyView browser fixture
    fixture.tsx             typed current gateway view cases
  desktop/
    CHECKLIST.md            what we always test and reproduce, with the contract each item holds
    evidence/
      conversation-unread/  screenshots the pull request shows for that check
    fixtures/
      attachments-races/    real panel with controlled attachment host and scenario gateway effects
      onboarding-readiness/ real setup with stalled HTTP response and retry cases
      provider-sign-in/     real transcript with typed auth refusal and controlled login launch
      message-sync/         real window over controlled active and stalled gateway reads
      app-review/           real window over a fake gateway whose app's call asks for a review
    scripts/
      provider-sign-in.mjs compact login recovery geometry, keyboard launch and replacement
      attachments-races.mjs pending image admission and concurrent URL-drop refusals
      onboarding-readiness.mjs readiness deadline and retry in both browser engines
      run-all.mjs           every check, summarised
      committed-transcript.mjs history notices, permission controls, re-enable
      smoke.mjs             loads, sends, splits, Settings, overview; console errors
      perf-budget.mjs       the frame budget, production build, 4× CPU throttling
      safe-area.mjs         nothing painted under the window controls, per frame
      columns.mjs           no two columns overlap in any state of the side rail, sidebar, list and overview; the rail's toggle and "nessa Studio" hold still; a full view's workspace takes no keys
      load-fallback.mjs     the load fallback inside, and centred in, the visible panel
      gateway-states.mjs    the desktop app's window when it cannot read the gateway: says why, never the sample
      conversation-unread.mjs a listed conversation the window could not read: the transcript and the Agents peek say what (#433)
      gateway-window.mjs    the desktop app's window over a real gateway (#419, #574): its handshake, a conversation, a live turn, and the test server's MCP App inline (starts its own gateway and dev server; needs the agent, `--agent claude|codex`, signed in, or `--scripted` for the text-reply scenario and none; the app step needs the dev server)
      command-order.mjs     an agent's command drawn in order, bidi controls visible (#553): the card, the overview row, and the peek
      scripted-scenarios.mjs  a scenario through the gateway and the window (#510): permission, a mid-turn failure, cancel (signed out; `--mode prod` previews a production build)
      scripted-e2e.mjs      one signed-out command: the gateway-backed checks in Chromium and WebKit, one verdict line (`pnpm test:e2e:scripted`)
      drag.mjs              pane drag: pointer path, zones, cancels, selection
      focus.mjs             where the caret lands after each pane and dialog change
      responsive.mjs        approval card, composer controls and thinking control, column titles, Settings widths and Integrations narrow, a pane's home
      widgets.mjs           widget hosts: a card, its pane, the window, Escape's order, focus, drag over the window
      message-sync.mjs      active DOM delivery, stalled list/read independence and idle cost
      app-review.mjs        an MCP App's review: read while the app's call waits, drawn and answered, the card and the overview row naming the app (dev server)
      mcp-apps.mjs          MCP Apps: each place, tools/call allowed and refused, CSP, isolation, escapes, forgery, departures and departures-back (dev server: imports the host's own builder), teardown
      mcp-apps-gateway.mjs  MCP Apps over a real gateway (#384): the test MCP server's app, its reviews, refusals and release; with `--scripted`, also an app's message and context update, and no model key (starts its own gateway and dev server; needs the agent, `--agent claude|codex`, signed in, or `--scripted` for none)
      mcp-servers-gateway.mjs Settings › Integrations over a real gateway (#391): add, inspect, focus, toggle, rename, narrow, conflict, remove, reconnect, non-admin, and the chart's app from a server added there (starts its own gateway and dev server; done-when needs the agent signed in)
      lib/
        selectors.mjs       cloneable CSS selectors, selector builders, key chords, storage keys and known-harmless messages
        selectors.test.mjs   the CSS table crosses browser evaluation as plain data (#441)
        cli.mjs             options, stderr diagnostics, JSON result, exit status
        server.mjs          reuse/start the dev server (warmed before the first page, a reused one too), or build + preview production
        browser.mjs         launch Chromium/WebKit, seed preferences, collect errors
        workspace.mjs       open panes, read pane rects and focus, lift a pane, rect containment, a model rule or value read in the page
        workspace.test.mjs  a measured drop must change pane order and remove its carried copy (#370)
        safe-area.mjs       the per-frame safe-area sampler
        perf.mjs            rAF gaps, Long Animation Frames, long tasks, throttling, calibration
        perf.test.mjs       the frame budget's unrounded decision, and the LoAF sample clock (#369)
        run.mjs             the main every check shares
        apps.mjs            an MCP App's documents, read through Playwright's frames; the window's card for an app's review, by its whole head
        apps.test.mjs       apps.mjs's rules, no browser: one inline mount, the one locator for an app's review card, and the wait for it to go
        settings.mjs        Settings › Integrations reached, and its fit measured at a width (responsive.mjs, mcp-servers-gateway.mjs)
        cli.test.mjs        the scripts' own contract, no browser: arguments, exit status, run-all's sum
        gateway-stack.mjs   a real gateway, the dev server or a production preview before it, and a client on it, for the real-gateway checks, or the scripted agent signed out; the panel's credential; an agent turn sent and waited out
        scripted-evidence.mjs the scripted command's verdict line and the pull-request summary; page lines are read from the checks' own results
        fake-host.mjs       the desktop app's host over IPC, faked: its gateway endpoint and credential answers
        gateway-view.mjs    a real gateway's view, for mcp-apps-gateway.mjs and gateway-window.mjs: setup's one admitted call, the review a step opened and how long that step waits for it (#474), what a text-only turn said
        gateway-view.test.mjs  gateway-view.mjs's rules, no gateway (#384's design table, and #419's W2–W3)
        browser.test.mjs    browser.mjs's recording of a failed request, no browser: harmless or an error (#485's F1′, F2–F4), and the favicon
```

When the UI moves, edit `lib/selectors.mjs` — nothing else names a class, a
label, a key or a storage key. A script that cannot find what a step needs to
begin says "could not run" for that step and names the selector, rather than
guessing; a step that began and then waited in vain for the product has
failed. Steps wait on conditions (`until`, `settled`, `contentIs`,
`paneCountIs` in `lib/workspace.mjs`), not on fixed times; where a check
asserts that something does not happen, its window is named and explained.

## Prerequisites

- `pnpm install` (brings the pinned `playwright` devDependency).
- Google Chrome installed (the default Chromium channel; it has Long Animation
  Frame timing). Or pass `--channel bundled` after `pnpm exec playwright install chromium`.
- WebKit for Playwright: `pnpm exec playwright install webkit`. On Linux, WebKit's system libraries: `pnpm exec playwright install-deps webkit`.
- `pnpm test:e2e:scripted` also needs a Rust toolchain (`cargo build -p nessa-server`, which the command runs) and the Playwright browsers above. It does not need a harness's `node_modules` or an owner credential: the gateway runs the scripted agent signed out. `scripts/remote/prepare.sh` from #358, on the parked branch `claude/358-remote-builds` and not on `main`, is what installs harnesses for the live agents; this command does not run it.
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
pnpm test:e2e:scripted -- --evidence /tmp/scripted-e2e
pnpm test:e2e:scripted -- --channel bundled --evidence /tmp/scripted-e2e
pnpm test:e2e:scripted -- --mode prod --evidence /tmp/scripted-e2e-prod
node verification/desktop/scripts/<check>.mjs --help
```

The stand-alone `attachments-races.mjs` and `onboarding-readiness.mjs` checks
start and stop their own controlled Vite fixtures and run both bundled Chromium
and WebKit. Run them directly with `node`; they do not use the app URL or the
`run-all` options. See their contracts in [the checklist](desktop/CHECKLIST.md).
When WebKit libraries live outside the system search path, set
`NESSA_VERIFICATION_WEBKIT_LIBRARY_PATH` to their directory; only WebKit receives
that library-path override, so Chromium keeps its normal runtime environment.

The shared desktop checks take `--url`, `--mode dev|prod`, `--engine chromium,webkit`,
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
- **exit** (`statusOf`, `lib/cli.mjs`): `0` every assertion held; `1` an
  assertion failed — whatever else could not run; `2` nothing failed but
  something could not run (the server did not start, a browser is missing,
  what a step needs to begin was not on the page, an option or `--only` name
  the check does not have). `run-all` sums its checks by the same rule.
  `--mode prod` leaves out checks that need the dev server and names them
  "not run: dev server only"; those rows are not "could not run".
- **the scripts' own tests**: `pnpm verify:desktop:test` (no browser). CI
  runs them in the frontend job, through `pnpm frontend:check`.

Per [machine-readable command output](../CODING_STANDARDS.md#machine-readable-command-output).
