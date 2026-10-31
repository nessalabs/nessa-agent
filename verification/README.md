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

### Concurrent dev servers

`startDevServer({ isolateCache: true }, env)` gives its Vite process a private
temporary dependency cache. Existing callers keep Vite's default cache. Use
the option when an external verifier starts another server from this checkout;
pass `NODE_ENV: "development"` for a development React runtime. The option
does not change the environment or settings of another server.

| Event | Cache owner and ordering | Regression |
| --- | --- | --- |
| Opt-in launch | Allocate a fresh directory and pass it as Vite's inline `cacheDir` | `server-cache.test.mjs`: separate real Vite/React optimizers preserve the canonical cache/runtime |
| Default launch | Retain the existing CLI and shared cache | Same test's production contamination control |
| Close or startup failure | Stop and await the owned process, then remove its private directory | `server-cache.test.mjs`: delayed process exit and failed configuration |

These tests prove cache isolation, not the cause of an earlier browser error.

The conversation stress review-answer observer and its request/ACK ordering
controls are mapped in [stress review answer evidence](../docs/design/stress-review-answers.md).
A review disappearing does not confirm an answer, and a control ACK does not
confirm provider tool completion.

Stress access scenarios name their fixture requirements explicitly. `full`
(the default) requires the selected model to advertise Ask, Auto and Full;
its existing success assertions are unchanged. `ask-only` requires only Ask
and replaces the counted access switches with Full-refusal checks, followed
by real permission turns that still require a review. It is separate evidence,
not Full-success evidence for that model. The published gateway catalog and
initial conversation must both match the exact requested model and scenario;
an incompatible fixture fails setup without selecting a fallback.

For example, on Linux run `conversation-stress.mjs --agent codex --model
gpt-6-astra --accessScenario full` for the Full-capable profile, and `--agent
codex --model gpt-5.6-terra --accessScenario ask-only` for the Ask-only profile.
Raise the same scenario with `--rounds 66 --reviews 30 --switches 9
--parentstops 3 --followers 4 --restarts 2`. The evidence keeps the exact
profile, requirements, review ACKs and typed access refusals. Availability
comes from the product; these examples do not enable additional presets.

A large-workspace measurement is specified in
[UI workspace load](../docs/design/ui-workspace-load.md). The seeded page is
opt-in (`workspace-load.mjs`). Startup, the pane cap, and one long transcript
are opt-in too (`alpha-perf.mjs`). Neither is part of `pnpm verify:desktop`.

## Layout

```
verification/
  README.md                 this file
  hook-behavior/            real harness hook events and outputs; see that README
  conversation/
    README.md               committed transcript fixture and publication ownership
    fixture.html            real ConversationControls and applyView browser fixture
    fixture.tsx             typed current gateway view cases and question-target controls
    vite.config.ts           verification-only production build of those components
    lifetime-fixture.html   restored tab over a gated original close
    lifetime-fixture.tsx    production follow/control consumers
    lease-fixture.html      the production details sheet over one published lease per page
    lease-fixture.tsx       each lease state the gateway publishes
  desktop/
    CHECKLIST.md            what we always test and reproduce, with the contract each item holds
    evidence/
      conversation-unread/  screenshots the pull request shows for that check
    fixtures/
      settings-action-owners/ production native-hook/notice rows with controlled pending sources
      sandbox-probes/      labelled host for visible isolation and navigation probes
      attachments-races/    real panel with controlled attachment host and scenario gateway effects
      onboarding-readiness/ real setup with stalled HTTP response and retry cases
      provider-sign-in/     real transcript with typed auth refusal and controlled login launch
      message-sync/         real window over controlled active and stalled gateway reads
      app-review/           real window over a typed fake gateway whose app's call asks for a review
      clock-snapshots/      public ClockProvider/useNow with injected time crossing a bucket between React reads
    scripts/
      settings-action-owners.mjs pending settings admission, rereads and source retirement
      page-load-regression/ synthetic host diagnostic reproduction; see its README (#916)
      provider-sign-in.mjs compact login recovery geometry, keyboard launch and replacement
      attachments-races.mjs pending image admission and concurrent URL-drop refusals
      onboarding-readiness.mjs readiness deadline and retry in both browser engines
      run-all.mjs           every check, summarised
      committed-transcript.mjs history notices, permissions, re-enable and question-target identities
      local-tab-lifetimes.mjs restored local identities, follow and completion ownership
      lease-details.mjs     the details sheet's Where it runs, for every published lease state
      smoke.mjs             loads, sends, splits, Settings, overview; console errors
      icon-buttons.mjs      every icon button: size, shape, name, tooltip, hover, keyboard focus
      shared-controls.mjs   key caps: one kit component, measured
      perf-budget.mjs       the frame budget, production build, 4× CPU throttling
      alpha-perf.mjs        cold/warm startup, the pane cap, one long transcript; --with-gateway seeds a scripted gateway (opt-in; not in run-all)
      safe-area.mjs         nothing painted under the window controls, per frame
      columns.mjs           no two columns overlap in any state of the side rail, sidebar, list and overview; the rail's toggle and "nessa Studio" hold still; a full view's workspace takes no keys
      load-fallback.mjs     the load fallback inside, and centred in, the visible panel
      gateway-states.mjs    the desktop app's window when it cannot read the gateway: says why, never the sample
      conversation-unread.mjs a listed conversation the window could not read: the transcript and the Agents peek say what (#433)
      gateway-window.mjs    the desktop app's window over a real gateway (#419, #574): its handshake, a conversation, a live turn, and the test server's MCP App inline (starts its own gateway and dev server; needs the agent, `--agent claude|codex`, signed in, or `--scripted` for the text-reply scenario and none; the app step needs the dev server)
      command-order.mjs     an agent's command drawn in order, bidi controls visible (#553): the card, the overview row, and the peek
      scripted-scenarios.mjs  a scenario through the gateway and the window (#510): permission, a mid-turn failure, cancel (signed out; `--mode prod` previews a production build)
      resize-observers.mjs  native ErrorEvent and pageerror checks on initial pane fitting and viewport changes, Chromium and WebKit
      scripted-e2e.mjs      one signed-out command: native resize and gateway-backed checks in Chromium and WebKit, one verdict line (`pnpm test:e2e:scripted`)
      boat-runtime-settings.mjs global lifetime default and saved-channel override/reset, ordinary config persistence and invalid request refusals through a real gateway, both engines/layouts; no Boat token or cloud VM
      drag.mjs              pane drag: pointer path, zones, cancels, selection
      focus.mjs             where the caret lands after each pane and dialog change
      lib/overview-keyboard.mjs atomic observation/action for incrementally mounted overview rows
      responsive.mjs        approval card, composer controls and thinking control, column titles, Settings widths and Integrations narrow, a pane's home
      widgets.mjs           widget hosts: a card, its pane, the window, Escape's order, focus, drag over the window
      message-sync.mjs      active DOM delivery, stalled list/read independence and idle cost
      app-review.mjs        an MCP App's review: read while the app's call waits, drawn and answered, the card and the overview row naming the app (dev server)
      lib/mode-content.mjs  compares semantic view content independently of approval mode, revision and observation clock; bare-Node tests preserve all remaining fields (#827)
      approval-deadline.mjs an armed review's countdown on the approval card, then the automatic notice at expiry (dev server)
      clock-snapshots.mjs   a public clock hook across a published tick and last-subscriber cleanup (dev server)
      mcp-apps.mjs          MCP Apps: each place, tools/call allowed and refused, CSP, isolation, escapes, forgery, departures and departures-back, the test server's chart app going live (dev server: the sandbox proxy starts in configureServer, so a preview cannot mount the fixture and run-all --mode prod leaves the check out; departures, departures-back and chart import the host's own builder), teardown, moving-frame action settlement
      mcp-apps-gateway.mjs  MCP Apps over a real gateway (#384): the test MCP server's app, its reviews, refusals and release; with `--scripted`, also an app's message and context update, and no model key (starts its own gateway and dev server; needs the agent, `--agent claude|codex`, signed in, or `--scripted` for none)
      mcp-servers-gateway.mjs Settings › Integrations over a real gateway (#391): add, inspect, focus, toggle, rename, narrow, conflict, remove, reconnect, non-admin, and the chart's app from a server added there (starts its own gateway and dev server; done-when needs the agent signed in)
      lib/
        selectors.mjs       cloneable CSS selectors, selector builders, key chords, storage keys and known-harmless messages
        selectors.test.mjs   the CSS table crosses browser evaluation as plain data (#441)
        cli.mjs             options, stderr diagnostics, JSON result, exit status
        server.mjs          reuse/start the dev server (warmed before the first page, a reused one too), or build + preview production
        browser.mjs         launch Chromium/WebKit, seed preferences, keep page diagnostic queues private
        resource-read-observer.mjs browser factory decorators: original consumed stream/client/mount observations, bounded transport and cutoff
        resource-read-liveness.test.mjs accepted/live ordering, later concurrent reads and original-owner stop/replacement controls
        resource-read-evidence.mjs Node incremental hash, exact Request correlation and bounded evidence ownership (#996)
        resource-read-modules.mjs opt-in exact Vite export decoration before navigation
        open-failure-evidence.mjs failure-only animation/RAF and redacted pending HTTP/RPC capture before disposal (#981)
        stress-conversation.mjs open exact conversation IDs; read Home's focused selected-row identity
        stress-conversation.test.mjs duplicate titles, delayed selection and missing identities cannot redirect selection
        stress-restart.mjs   scope intentional restart console lines to exact socket evidence and verified recovery (#1024)
        stress-restart.test.mjs temporal, endpoint, occurrence, failed-recovery and cleanup boundaries
        stress-followers.mjs subscription churn and capacity probes, correlated with exact run-owned deletions
        stress-followers.test.mjs end/open ordering, cursor resume, real held counts and deletion/capacity controls
        page-lines.mjs      report each collected page event through its step or close owner
        page-lines.test.mjs queue-free page API, classification, once-only reporting and concurrent attribution
        workspace.mjs       open panes and completed tool work, read pane rects and focus, lift a pane, rect containment, a model rule or value read in the page
        workspace.test.mjs  a measured drop must change pane order and remove its carried copy (#370)
        safe-area.mjs       the per-frame safe-area sampler
        perf.mjs            rAF gaps, Long Animation Frames, long tasks, throttling, calibration
        perf.test.mjs       the frame budget's unrounded decision, and the LoAF sample clock (#369)
        run.mjs             the main every check shares
        apps.mjs            an MCP App's documents, read through Playwright's frames; the window's card for an app's review, by its whole head
        apps.test.mjs       apps.mjs's rules, no browser: one inline mount, the one locator for an app's review card, and the wait for it to go
        settings.mjs        Settings › Integrations reached, and its fit measured at a width (responsive.mjs, mcp-servers-gateway.mjs)
        settings-tabs.mjs   smoke's visible tab strip compared with the Settings catalogue's own visibility and order; settings-tabs.test.mjs rejects missing, reordered and unexpected tabs
        review-notices.mjs  Settings notification permission and accessible Allow-button evidence
        review-notices.test.mjs default/denied/granted/unsupported row contracts, without a browser
        cli.test.mjs        the scripts' own contract, no browser: arguments, exit status, run-all's sum
        gateway-stack.mjs   a real gateway, the dev server or a production preview before it, and a client on it, for the real-gateway checks, or the scripted agent signed out; the panel's credential; an agent turn sent and waited out
        stress-profile.mjs  explicit stress scenario/model requirements and refusal provider-effect checks; stress-profile.test.mjs covers incompatible fixtures and counted Ask-only work
        acp-recording.mjs   complete JSONL and JSON-RPC envelope validation shared by parent Stop and access-refusal evidence
        stress-access-refusals.test.mjs exact window Full-request refusal correlation, sharing the review command observer
        scripted-evidence.mjs the scripted command's verdict line and the pull-request summary; page lines are read from the checks' own results
        fake-host.mjs       the desktop app's host over IPC, faked: its gateway endpoint and credential answers
        gateway-view.mjs    a real gateway's view, for mcp-apps-gateway.mjs and gateway-window.mjs: setup's one admitted call, the review a step opened and how long that step waits for it (#474), what a text-only turn said
        gateway-view.test.mjs  gateway-view.mjs's rules, no gateway (#384's design table, and #419's W2–W3)
        browser.test.mjs    browser.mjs's recording of a failed request, no browser: harmless or an error (#485's F1′, F2–F4), and the favicon
```

`resource-read-regression.mjs` is an opt-in harness regression using
`desktop/fixtures/resource-read/index.html`: real production factories over
synthetic HTTP/ports in Chromium and WebKit, deterministic simulated abort
classification, corrupt/truncated/canceled/duplicate controls and a deliberate
finite-animation timeout. Run it with `--channel bundled --evidence <new-dir>`.
The actual app flow is separately required by `gateway-window.mjs`'s dev apps
step. See [resource-read evidence](desktop/resource-read-evidence.md) for bounds
and the difference between a synthetic abort and a natural browser failure.

When the UI moves, edit `lib/selectors.mjs` — nothing else names a class, a
label, a key or a storage key. A script that cannot find what a step needs to
begin says "could not run" for that step and names the selector, rather than
guessing; a step that began and then waited in vain for the product has
failed. Steps wait on conditions (`until`, `settled`, `contentIs`,
`paneCountIs` in `lib/workspace.mjs`), not on fixed times; where a check
asserts that something does not happen, its window is named and explained.

The standalone MCP sandbox departure and chart checks use
`desktop/fixtures/sandbox-probes/index.html`. Their visible frames stay on that
labelled verification page, separate from the product UI. Every check releases
its message-listener owner and removes its frames before closing the page;
cleanup assertions report any remaining frames or product markup.

## Hook behavior

`hook-behavior/` drives a real harness and records which hook events fire.
Claude needs the login on the machine that holds it. Codex and OpenCode
install their pinned packages and talk to a scripted model. How to run them,
and where the JSONL and summary go, is [hook-behavior/README.md](hook-behavior/README.md).
They are not a desktop check and not a CI gate.

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

## Scenario results and preview ownership

### Focus and drag observation order

The focus and sideways-drag probes keep readiness and timing evidence separate
(`lib/workspace.test.mjs`). No production deadline is changed by these probes.

| Observed ordering | Result |
| --- | --- |
| Selected overview row focused, later rows pending | Focus is ready; Down waits for a mounted successor within 2,000 ms. |
| List complete but focus missing, outside the list, or no successor for Down | Focus/navigation is not ready. |
| Complete-list marker absent after 2,000 ms | Independent `focus-overview-complete-list` result fails, even if focus held. |
| Pointer delivered and zone observed before the event's 150 ms rest boundary | The moving-sweep assertion checks the announced zone. |
| Pointer delivery or DOM observation reaches the rest boundary first | No proof of moving intent; missing timely evidence fails and records both gaps. |
| No announcement, or announcement before any recorded changed-position move | No proof of moving intent; the sweep fails. |

`recordZones` retains event time, event delivery time and DOM observation time.
The last is after a scheduled preview, not the model's decision time. A later
legitimate rest announcement stays in the receipt without replacing a timely
moving observation. Historical late announcements alone cannot identify whether
input delivery, rendering, host scheduling or another cause was responsible.

A scenario owns its result; a missing optional assertion list cannot replace the
scenario's original error. `report.add` normalizes `failures` after accepting the
result fields, then derives `ok` and the existing process verdict. Page diagnostic
lines still belong to the existing page-lines owner.

A check that needs a source fixture in production owns the preview that includes
that fixture. `run-all` delegates that choice through `ownsPreview` and passes the
production mode rather than substituting its ordinary application preview.
Settings uses `desktop/fixtures/native-hooks-on/vite.config.ts`, which extends the
application build with the controlled Settings HTML input. An explicit `--url`
remains externally owned: it starts no server and must already serve the fixture.

| Row | Input or ordering | Required result |
| --- | --- | --- |
| V1 | Result omits `failures`, or supplies `undefined`, with no error | Normalize to an empty assertion list; derive `ok: true` and exit 0 |
| V2 | Result supplies `failures: undefined` alongside an original scenario error | Preserve that error in the entry and stderr; derive `ok: false` and exit 1 rather than throwing from the reporter |
| V3 | Assertions fail alongside an error, or an explicit input `ok: true` contradicts them | Preserve both diagnostics; derive false and exit 1 from the owned evidence |
| V4 | Result cannot run, or is intentionally skipped, with missing assertions | Keep the existing distinction: cannot-run is not passed even without a diagnostic and exits 2; a skipped row is not passed and does not count toward a successful run |
| V5 | Standalone Settings uses `--mode prod` without a URL | Build its application-plus-Settings fixture inputs and close its own preview |
| V6 | `run-all --mode prod` schedules Settings | Let Settings own that same preview; pass production mode without a shared application URL |
| V7 | Settings receives an explicit URL in standalone or full-run mode | Forward the exact URL; leave server/build lifetime with its caller, including when mode is explicitly production |
| V8 | Ordinary functional check runs through `run-all` | Keep the shared page URL and its actual dev/production mode |
| V9 | Settings runs in dev mode, or an existing preview owner runs | Settings uses the source-serving dev page; committed-transcript, message-sync and perf retain their established preview ownership |
| V10 | Full-run `--runs` accompanies fixture-owned functional checks and perf | Forward the run count only to perf-budget, whose CLI owns that option; preserve Settings and other functional argument validity |
| V11 | Settings reads the browser notification permission before checking its row | Default offers exactly one accessible Allow button and explains waiting reviews; denied shows blocked copy with no Allow button, even though the copy mentions Allow; granted shows waiting-review status with no Allow button; unsupported has no notification row |

Bare-Node reporter/routing tests cover V1–V4 and the full-run argument decisions.
V5–V9's actual fixture and server lifetime also require production browser
qualification; helper tests alone do not establish that the built fixture renders.
No frame budget, RPC deadline or browser readiness wait changes with this routing.

## Stress follower end/open ordering (#1038)

The follower owns desired targets separately from live subscription handles.
The generated `ConversationSubscriptionEndReason` contract owns recoverability:
`source_closed` and `lagging` subscribe again, as does the client's
`disconnected`. A view resumes from its applied frame cursor, events from their
applied batch cursor, and a list receives a replacement snapshot. End callbacks
retire ownership even when they report an unexpected failure. Opening identity
is shared across all three stream kinds; data and end callbacks from a retired
opening cannot change its successor. `stress-followers.test.mjs` exercises these
orderings and the existing exact deletion/capacity controls.

| State/event | Ownership and next action |
| --- | --- |
| Opening receives its handle | Retain it only while that opening is current |
| Recoverable end before or after the open answer | Retire the opening and handle, preserve applied data/cursor, request reopening; close a late handle |
| Unexpected end | Retire identically, retain the failure, and request reopening without erasing that failure |
| Retired opening delivers data/end | Ignore it; its successor owns application progress |
| Intentional churn | Retire before awaiting close, then reopen from the applied cursor |
| Run-owned deletion during events admission | Retire and close any late handle; remove the desired target |
| Missing or opening list/view/events | Defer capacity probes; desired targets still reserve their room for reopening |
| An owned stream ends while a probe awaits admission | End that measurement and close probe handles; a later stable round measures capacity |
| Settlement after churn ends | Reopen list, view and events before comparing final state |

`held()` counts live conversation handles, while desired targets reserve room
independently. Lists have their separate published allowance. These rules do not
change server capacity, approval policy, concurrency or deadlines.

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

The app-review fixture contract and ordering evidence are in
[app review verification](../docs/design/app-review-verification.md).
