# Desktop verification checklist

What we always test and reproduce on the desktop window. Each item names the
contract it holds (and where that contract is written), how to check it — a
script under [`scripts/`](scripts/) or manual steps — and the noise known to
be harmless. Why these exist and when to run them:
[Browser verification for UI](../../CODING_STANDARDS.md#browser-verification-for-ui).
How to run them: [`verification/README.md`](../README.md).

ADR 238 is [`docs/adr/done/238-desktop-workspace-frontend.md`](../../docs/adr/done/238-desktop-workspace-frontend.md);
its sections are cited as _ADR 238 › Section_. The window as a whole is
described in [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md) (_Minimal
desktop surface_ and the workspace section).

Every scripted item runs in **Chromium and WebKit** unless it says otherwise.
The app ships in WKWebView; Chrome alone is not evidence.

## Seeded workspace load

Opt-in. Not part of `run-all`. The contract is
[UI workspace load](../../docs/design/ui-workspace-load.md).

- [ ] **Rendered count matches the generated count** on the Agents overview
  after Show All, on the columns session list for the opening channel, and
  on the sidebar branch after Show all. The collapsed branch is short of
  that channel. The page does not call `conversation.list`.
  _Check:_ `workspace-load.mjs` (defaults: `--mode prod`, the dry run of
  10,000 sessions). Chromium records split and drag frames through
  `lib/perf.mjs` at 4× after calibration. A frame over 50 ms is a finding,
  not a failure of this check. WebKit runs the journeys without that throttle.

## Alpha startup and stress

Opt-in. Not part of `run-all`. It does not change the frame budget and it
does not replace `workspace-load.mjs`.

- [ ] **Cold and warm startup are measured on the sample workspace**, in
  Chromium and WebKit, production build. Cold is a fresh context (Chromium
  disables the HTTP cache). Warm is the second reload after a cache fill.
  The row keeps ready time, Navigation Timing, first contentful paint, heap
  when the browser exposes it, and the longest rAF gap. A missing navigation
  entry or a startup screen still up fails the row. There is no startup
  budget.
  _Check:_ `alpha-perf.mjs` (`startup`).
- [ ] **The layout's pane cap is filled, and one long seeded transcript
  scrolls.** The cap is `paneLimits.maxPanes`. One further split does not
  add a pane. The long transcript is 24 sessions, one transcript of 24
  messages, 4,000 ASCII characters in the plain part. Chromium records the
  refused split and the scroll at 4× CPU throttle. A frame over 50 ms is
  stored and does not fail the row. A pane past the cap, a short fill, a
  split chord that does not arrive, or a transcript that does not overflow,
  fails the row. A missing frame sample does not replace that result.
  _Check:_ `alpha-perf.mjs` (`panes`, `transcript`).
- [ ] **A scripted gateway is stressed only with `--with-gateway`, in
  Chromium and WebKit, columns.** The gateway stores `paneLimits.maxPanes`
  conversations. One of them has four copies of a 4,000-character user
  message; the scripted agent answers each with text. The scroll is
  recorded only after those replies are on the page. A transcript of the
  user lines alone fails the row. Cold and warm each
  ask the host for the endpoint, and the ready mark is that long
  conversation's session row, so the clock includes the list. The same
  window fills the pane cap and scrolls that transcript. `conversation.list`
  has to be complete (`seedHeld`); an incomplete list is not measured.
  This is not a catalogue walk.
  _Check:_ `alpha-perf.mjs --with-gateway` (`gateway-seed`, `gateway-startup`,
  `gateway-panes`, `gateway-transcript`).
- [ ] **Overview, drag, and split budgets stay in `perf-budget.mjs`.** A number
  there that misses 50 ms and matches `#588` / `#606` is that known result.
  The 10,000-session dry run stays in `workspace-load.mjs`.

## Performance budget

_ADR 238 › Context_: "Calm means no dropped frames" — no frame over 50 ms, in a
production build at 4× CPU throttling.

- [ ] **Every budgeted interaction stays under 50 ms per frame**, in both
  layouts: split right, split down, move, close, sidebar toggle, session-list
  toggle, send from a new session, typing, streaming into one of four panes, a
  pane dragged across zones (dropped, and cancelled), the Agents overview
  opened, walked, answered and left, and the composer's thinking control
  walked from its least level to its most and its knob dragged along the track.
  _Check:_ `perf-budget.mjs` (defaults: `--mode prod --throttle 4 --runs 3`).
  Read max and median per row; every over-budget frame carries its Long
  Animation Frame attribution (scripts, forced layout, style-and-layout time).
  The JSON also retains the original sample origin, raw rAF gaps and trusted
  input timestamps/targets to distinguish preparation from the interaction;
  that evidence does not filter frames or change the budget decision.
- [ ] **Frame gaps use callback execution time.** Sampling `performance.now()`
  inside rAF keeps delayed callbacks, the interaction origin and attribution
  on one clock. A nominal frame timestamp must not shorten known blocking work.
  _Check:_ `lib/perf.test.mjs` (delayed timestamps and a zero first timestamp),
  `perf-budget.mjs` (known-cost calibration).
- [ ] **The budget decides on the unrounded frame** (#369). The table rounds
  maxima for display. A 50.1 ms frame fails when that table shows 50; an exact
  50 ms frame and a 49.6 ms frame pass.
  _Check:_ `lib/perf.test.mjs` (`frame budget`).
- [ ] **Style-and-layout time is a duration on the performance clock** (#369). The
  sample shifts `start`, `renderStart`, and `styleAndLayoutStart` together
  onto the interaction. A zero phase start means that phase did not run, so
  the diagnostic is absent rather than a negative duration.
  _Check:_ `lib/perf.test.mjs` (`long animation frame clock`).
- [ ] **The ambient grain is a baked image, not a runtime noise filter** (#370).
  Its 160×160 PNG tile repeats with 0.06 opacity and normal blending — overlay
  blending would repaint the window under the veil. Keeping the texture in
  `ui/ambient-grain.png` avoids SVG turbulence in the GPU raster path
  (`src/desktop/styles.css`, `.desktop-grain`).
  _Check:_ `smoke.mjs` (`ambient-grain`), both engines/layouts; `perf-budget.mjs`
  checks the unchanged frame budget.
- [ ] **The measurement works.** The calibration busy loop slows by roughly
  the throttle rate, and a 120 ms frame of known cost is measured and attributed.
  _Check:_ the `calibration` result of `perf-budget.mjs`; if it fails, no
  number below it means anything.
- [ ] **Each interaction did what it is named for** (a split added a pane, a
  move reordered, an answer removed a request), so a no-op cannot pass as fast.
  _Check:_ `perf-budget.mjs` fails a row with "did not do what it is named for".
- _Harmless:_ `--mode dev` numbers (StrictMode double effects, unminified
  code) are indicative only. A single headless outlier on one run is re-run
  `--headed` and with more `--runs` before it is reported.

## Titlebar safe area

_ADR 238 › The titlebar's safe area_: nothing is painted under the window's
controls — text, icons, images, code, or decorative art — in any frame.

- [ ] **Nothing painted under the traffic lights in any frame** across ⌘B,
  ⌥⌘S, the edge peek, a sidebar edge dragged to collapse and back, a resize
  through the breakpoints, a split and a close, Settings (open, ⌘B, peek,
  leave, and leaving with the sidebar hidden), a layout switch, a scrolled
  transcript, and the Agents overview — at 1440 × 900 and 1000 × 700.
  _Check:_ `safe-area.mjs`. It first proves the sampler catches a label placed
  at (8, 8) (`sampler-control`).
- [ ] **The titlebar's controls and the traffic lights share one row**: the
  controls are centred 24px below the window's top in every frame of ⌘B, rail
  open and closed, and the host centres the native buttons on that same line.
  _Check:_ `columns.mjs --only titlebar-row` for the page;
  `cargo test -p nessa-app --no-default-features --test traffic_lights` for the
  buttons in a real AppKit window (macOS) and the row height the two share.
- [ ] **Column titles sit inline after the controls when they fit, below when
  they do not**, and an inline title never overlaps a titlebar control.
  _ADR 238 › The titlebar's safe area_ (`titlePlacement`).
  ResizeObserver delivery measures only; the latest placement commits before
  the next paint, with no ResizeObserver loop error (#693).
  _Check:_ `responsive.mjs --only column-title`; drag a column edge slowly
  across the flip point by hand and watch for a flicker (the 12px hold).
- [ ] **A column's titlebar action hides at once as it folds and shows once it
  has slid in; a pane's header stays painted as workspace bounds land.**
  _Check:_ manual, with
  `--headed` and a slowed animation (DevTools › Animations at 10%).
- _Harmless (exempt by design):_ titlebar rows themselves, tooltips and the
  hidden title sizer. Decorative art is **not** exempt: the picture band
  (`[data-sliver]`) and the night scene begin after the controls in the corner
  pane, including as workspace bounds change. Nor is the drag's copy: its
  layer clips it below the titlebar row (`drag.mjs`, `copy-under-controls`).

## Drag and drop

_ADR 238 › Drag and drop_ — its table (every phase, every event) and its
zone table are the contract; these are the rules a script holds, in Chrome
and WebKit, both layouts, 1440 × 900 and 1000 × 700:

- [ ] **One aim point: the pointer.** Once lifted, the copy glides until its
  centre is under the pointer and stays there; what the eye tracks is what
  aims. _Check:_ `drag.mjs` (`sweep-across-zones`, follows-pointer).
- [ ] **Every preview stays inside the grid and the window, every frame.**
  _Check:_ `drag.mjs` (`sweep-across-zones`, inside-grid and inside-window).
- [ ] **Panes move one way between zone changes.** _Check:_ `drag.mjs`
  (`sweep-across-zones`, one-way).
- [ ] **Zones are direction-aware**: a sideways sweep near a tall, narrow
  pane's top reaches its side, never above or below. _Check:_ `drag.mjs` (`sideways-top`).
- [ ] **A zone holds at its boundary** (another must win by 12px): ±6px
  jitter does not flicker it. _Check:_ `drag.mjs` (`boundary-jitter`).
- [ ] **The zone settles at rest**: approached fast, then still for 300 ms, a
  1px nudge changes nothing; a release after a pause drops on the zone at
  rest. _Check:_ `drag.mjs` (`rest-settles`); unit tests `split-panes/model/drag.test.ts`.
- [ ] **The middle of a tall pane offers Swap.** _Check:_ `drag.mjs` (`swap-in-tall-pane`).
- [ ] **Escape, or a release outside the window, cancels**: the copy flies
  home, the layout is unchanged, nothing is left lifted or transformed.
  _Check:_ `drag.mjs` (`escape-cancels`, `outside-cancels`). If an engine
  delivers no pointer event outside the viewport, the script says it could
  not run — check that release by hand in the app.
- [ ] **A lost pointer ends the press and the drag**: a move and a release
  over a zone after it start and drop nothing. _Check:_ `drag.mjs` (`lost-capture-then-move`).
- [ ] **Any change while carrying cancels at once** — a resize, ⌘W, ⌘0 (and
  every other command key, the store changing, Settings): nothing is left
  lifted right after, every pane stays inside the window every frame, nothing
  is painted under the controls, and the release drops nothing. _Check:_
  `drag.mjs` (`resize-mid-drag`, `command-mid-drag`); the rest in unit tests
  (`split-panes/adapters/dom/drag.test.tsx`,
  `workspace/adapters/dom/split-panes-drag.test.tsx`).
- [ ] **Only panes a person can see are targets**: a session carried while
  the overview covers the panes offers no zone and no placeholder, and its
  release changes nothing. _Check:_ `drag.mjs` (`overview-session-drop`, sessions in the sidebar).
- [ ] **A side column is never a target, and the revealed sidebar stays for
  the drag**: a session carried inside the sidebar revealed from the edge
  offers no zone and no placeholder, the reveal does not hide while the button
  is held, and the release changes nothing. _Check:_ `drag.mjs`
  (`peek-session-no-zone`, sidebar layout, both engines).
- [ ] **A pane carried over a docked side column finds no zone**: over the
  docked sidebar and the docked session list, no zone and no placeholder; the
  release there changes nothing. _Check:_ `drag.mjs` (`docked-columns-no-zone`,
  1440 × 900, both layouts).
- [ ] **A press freezes the edge reveal** (`src/desktop/model/edge-peek.ts`,
  its table): pressed within the reveal's 350 ms hide — the pointer just left
  it for a pane's header — it stays every frame of the drag, is no zone, and
  just past its edge a pane is a target; released away, it hides. A release
  the page missed, or the window's blur, ends the freeze. _Check:_ `drag.mjs`
  (`peek-press-while-hiding`); unit tests `edge-peek.test.ts`,
  `use-edge-peek.test.tsx`.
- [ ] **Only a drop that was previewed commits**: down, across and up in one
  task — before the copy is made, or lifted but before any preview — changes
  nothing. _Check:_ `drag.mjs` (`flick`). A release commits the zone shown as
  the button lifts, never one decided again then (a heading aging out as it
  lifts cancels nothing). _Check:_ unit tests `split-panes/model/drag.test.ts`,
  `split-panes/adapters/dom/drag.test.tsx`,
  `workspace/adapters/dom/split-panes-drag.test.tsx`.
- [ ] **Only the primary button carries**: the right button joining the left
  mid-drag ends the drag; nothing is carried after it and its release drops
  nothing. _Check:_ `drag.mjs` (`chord-right-button`).
- [ ] **Nothing carried paints under the window controls.** The compact
  card is clipped below the titlebar during every frame.
  _Check:_ `drag.mjs` (`copy-under-controls`).
- [ ] **No text selection is left behind**, during or after a drag.
  _Check:_ `drag.mjs` (`sweep-across-zones`, `outside-cancels`).
- [ ] **The shown zone commits the same drop outcome.** The target highlight
  identifies the gesture's zone; the model's proposal determines the final
  layout. _Check:_ unit test `panes.test.ts`; `drag.mjs` (`stationary-drag-target`).
- [ ] **The carried card stays compact.** Its row-sized box keeps the same
  dimensions over every target and its centre follows the pointer within 2px;
  its title never stretches. It contains no transcript, image band or composer.
  _Check:_ `drag.mjs` (`compact-drag-card`; `--shots <dir>` saves target views),
  `workspace/adapters/dom/split-panes-drag.test.tsx`.
- [ ] **Live chats stay still during targeting.** Their rects stay within 2px
  of their starting geometry, pane material and body visibility stay unchanged,
  and the highlight marks the hovered half or whole pane. Release commits the
  split; cancellation leaves the layout unchanged and removes owned resources.
  _Check:_ `drag.mjs` (`stationary-drag-target`; every drag check's residue).
- [ ] **Workspace bounds land immediately and leave chats live.** Closing or swapping
  panes keeps their bodies and material visible. The zero-duration workspace
  flight skips geometry snapshots, releases drag previews, and cancels any
  prior flight without delayed body restoration.
  _Check:_ `drag.mjs` (`stationary-layout-flight`, `stationary-drop-commit`),
  `split-panes/adapters/dom/flip.test.tsx` (zero-duration and interrupted flights).
- _Harmless, and not a failure:_ a single read of a title's transforms in
  WebKit that mixes two moments. How the two stretch checks read a title is
  `recordShapeFrames` (`scripts/lib/shape-sampler.mjs`), and why, with the runs and probes, is #365.
  Also harmless: one agreed read of about a percent (both axes within two
  percent of 1, and the two scales within four percent of each other) while
  every running animation on that title shares one start time. That is the
  box and its counter-scale sampled a step apart inside the frame (#254).
  A read whose animations started apart, a larger stretch, or a title with
  no running animation still fails, in both engines.
- [ ] **A zone the fit rule refuses offers nothing; a session already on
  screen offers "Go to Pane".** _Check:_ manual (drag a session from the list
  onto four panes; drag an on-screen session).

## Panes and the fit rule

_ADR 238 › One fit rule for every change of layout_.

- [ ] **Every pane at least 300 × 220 after any change of layout**; a named
  side is that side or nothing (Split Right never silently splits down).
  _Check:_ unit tests in `pane-sizing`; by hand: ⇧⌘N repeatedly at 1100–1400
  px, with and without the overview open — the split is measured against the
  room the panes will have.
- [ ] **The session list's content sits evenly in its column.** Its search
  field and a row's highlight are as far from what is on their left — the
  sidebar's card, or with the sidebar away the window's edge — as from the
  panes on their right. _Check:_ `responsive.mjs --only list-gutter`
  (Chromium and WebKit, sidebar open and closed).
- [ ] **Side columns fold first for room, and come back when there is room.**
  _ADR 238 › Side columns_ table. _Check:_ `safe-area.mjs --only resize-breakpoints`
  exercises it; watch with `--headed`.
- [ ] **Where even folding cannot fit them, composers go compact rather than
  clip.** _Check:_ `responsive.mjs --only composer-chips` (no control overlaps
  another down to 600 px).
- [ ] **Empty drafts are never listed.** _ADR 238 › Decision_ (`model/`):
  "a draft is never listed". _Check:_ `smoke.mjs` (`draft-unlisted`).
- [ ] **What is typed and not sent survives** a layout change, Settings, and
  the pane showing another session. _ADR 238 › What is typed and not sent_.
  _Check:_ manual.

## Side rail

_Prototype, `workspace/ui/chrome/side-rail.*`, off until Settings › Advanced ›
Experimental turns it on; the rail stands beside the workspace in
`.workspace-window`, not inside it. `columns.mjs` turns it on for its checks._

- [ ] **Off by default, and off means absent**: no rail, no toggle, the
  workspace at the window's edge. _Check:_ `columns.mjs --only rail-off`;
  `smoke.mjs` (Experimental offers the side rail's switch, off).

- [ ] **Focused panes keep the same fill and elevation as their neighbors.**
  A subtle rim identifies focus in both themes. _Check:_ `columns.mjs --only pane-level`.
- [ ] **Conversation content stays visible during toggles and when a pane mounts.**
  Pane bodies, including the composer, do not disappear and their fill does not
  darken as bounds land immediately, without a workspace FLIP. Independent
  sidebar and session-list motion remains. _Check:_ `columns.mjs --only content-visible`
  in both engines and layouts. Its split action opens a filled conversation and
  samples the new pane's body and header from their first mounted frame: full
  cumulative ancestor opacity, visible content and unit root/header transform.
  Drag previews must also retain content; verify
  these with the drag and focus checks when changing their motion.
- [ ] **No two columns overlap**, and nothing sits under the rail, in any
  combination of the rail, the sidebar, the session list and the Agents
  overview, at 1440, 1000 and 760 wide. _Check:_ `columns.mjs --only no-overlap`
  (Chromium and WebKit, both layouts).
- [ ] **The rail's toggle never moves**: it is at the control edge of the
  window's bottom-left corner in every state, and hidden exactly when the rail
  and the sidebar are both closed. **"nessa Studio" is at the same pixels in
  Agents and in an item's full view**, and never under the toggle.
  _Check:_ `columns.mjs --only rail-corner`.
- [ ] **Under an item's full view the workspace takes no keys**: ⌘B, ⌘0 and
  ⌥⌘S change nothing behind it. _Check:_ `columns.mjs --only rail-view-inert`.
- [ ] **An item's view opens empty, and a slow load shows only text**: no
  loader before 3 s; still loading at 3 s, the kit's `LoadingText` sweeps
  "Loading Files" with no panel or border around it. _Check:_
  `columns.mjs --only rail-loading`.
- [ ] **The open rail holds the traffic lights in its top row**: in the macOS
  window the lights never move; the open rail is wide enough for them, its
  items are centred under them, and so is its toggle, open or closed; the
  first card starts as far past the green button as the red one is from the
  window's edge. Open, closed or off, the titlebar's first control starts
  where the lights end. _Check:_ `conversation-stress.mjs` step `rail` (the
  fake host makes the page macOS; `columns.mjs` keeps the browser's rail);
  `side-rail.test.tsx` ties the rail's width to `traffic_lights.rs` and
  `styles.css`.
- [ ] **The titlebar's controls never pass under the window's controls while
  the rail opens or closes**: every frame draws them between where they were
  and where they land. _Check:_
  `columns.mjs --only titlebar-steady`.
- [ ] **Toggling the rail moves things by transform only**: the width lands at
  once and FlipScope plays the columns back, so no frame lays text out again.
  _Check:_ `perf-budget.mjs` (production build) — not yet run for the rail.

## Keyboard and focus

_ADR 238 › Focus follows the focused pane_; keys in
`src/desktop/workspace/ui/layouts/shortcuts.ts`.

- [ ] **The caret lands in the focused pane's composer** after ⌘N, ⇧⌘N,
  ⌘\\ and a pick, ⇧⌘\\, ⌘1–4, ⇧⌘] and ⇧⌘[, and ⌘W. _Check:_ `focus.mjs` (`focus-panes`).
- [ ] **Closing ⌘K without a pick, or Settings, gives focus back** to what
  opened it. _Check:_ `focus.mjs` (`focus-panes`).
- [ ] **Replacing a workspace layout retires queued focus settlement**: deliver
  held callbacks after the old workspace unmounts; they enqueue zero new frame
  work, and the current layout still focuses its composer. _Check:_ `focus.mjs`
  (`focus-retired-layout`); [frame lifetime](../../docs/design/ui-workspace-load.md#focus-frame-lifetime-730), #730.
- [ ] **Keys during incremental overview rendering use the mounted prefix.**
  The probe is armed before opening and performs End/Down/Home in the same
  callback that observes eligible rows, retaining its own measurements.
  _Check:_ `focus.mjs` (`focus-overview-arriving`), `lib/overview-keyboard.test.mjs`.
- [ ] **Bound workspace keys stay consumed while their actions are unavailable.**
  With the quick switcher open, ⌘N and ⌘W prevent the native default, leave the
  panes unchanged, and retain the switcher. An unbound key remains unconsumed;
  closing the switcher restores ordinary split commands.
  _Check:_ `focus.mjs` (`focus-bound-keys`), [workspace shortcuts](../../docs/design/workspace-shortcuts.md).
- [ ] **A burst of keys leaves a working window**: no error, a focused pane.
  _Check:_ `focus.mjs` (`focus-mash`).
- [ ] **A pane command from the overview**: one that is a navigation leaves
  it even when it changes nothing (⌘ and the focused pane's number, ⇧⌘[ at
  the first pane) and the caret lands in the focused pane's composer; one
  that changes nothing and is not a navigation (⌃⌥← at the edge) leaves the
  overview where it is. _Check:_ `focus.mjs` (`focus-overview`).
- [ ] **Both layouts answer the same keys with the same parts.** _Check:_
  unit test `layouts.test.tsx`; every script runs both layouts.
- _Harmless:_ in dev, StrictMode runs effects twice; if focus looks wrong only
  in dev, confirm with `--mode prod` before reporting.

## Agents overview

Header and group measurements wait for the published `data-overview-listed`
marker, including a reopened list; finite motion finishing does not mean its
deferred rows have all mounted. The sampling order is in
[UI workspace load](../../docs/design/ui-workspace-load.md#verification-sampling-order).

_ADR 238 › What fills the content region_ (the overview is workspace state).

- [ ] **It is always offered**: on a fresh profile, with nothing stored, the
  sidebar shows "Agents" (and no separate Needs you or Running rows) and ⌘0
  opens it. _Check:_ `smoke.mjs` (`overview`); unit test `layouts.test.tsx`.
- [ ] **⌘0 and the sidebar's Agents entry open it; asked again, it stays.**
  _Check:_ `focus.mjs` (`focus-overview`), `smoke.mjs` (`overview`).
- [ ] **The keyboard walks its items; ⌘↩ allows, ⌘⌫ denies, ⌘R replies**, and
  the caret stays in the overview after an answer. _Check:_ `focus.mjs`
  (`focus-overview`, the walk); `perf-budget.mjs --only overview-answer` (answer lands); ⌘⌫ and ⌘R by hand.
- [ ] **Each request shows its answers when pointed at or reached by the
  keyboard**: at rest a request row shows its title and time,
  its why and its command, and no answers — nor room kept for them: the why
  and the command run on to the time (the row's 12px gap before it, ±4).
  Pointed at, or with the keyboard visibly on it (`:focus-visible`), Deny and
  Allow Once (⌥ turns it into Always Allow where offered) take the time's
  place at its end, sliding in (at once with less motion); the row's height
  and the list do not change. A keyboard-focused row or answer button keeps
  its controls visible and its time hidden while the pointer explores another
  row; both rows can show controls. A row clicked and chosen without visible
  keyboard focus (its peek beside the list) shows none while the pointer is
  on another row or off the list. The
  answers carry no tooltip; each names its chord (`aria-keyshortcuts`);
  labels ≥ 4.5:1. _Check:_ `responsive.mjs --only overview-row-answers`
  (Chromium and WebKit); unit tests `overview.test.tsx`.
- [ ] **An answered request lands where the source now puts it**: no
  label on the row and no ring, box or line around it. Taken, the row waits
  quietly until the source moves its session on (at most 2s; then it shows
  as the workspace holds it). The final group and gap are placed immediately,
  without departing copies, blur, scale or list-item travel; unrelated row
  identities and visibility stay intact. Group changes and inline expansion
  use the same immediate list placement. A mounted conversation stays fully
  opaque through its ancestors from its first frame on opening, answering
  into another session, and inline expansion. Non-conversation affordances
  retain their own motion. Deny follows the same lifecycle. The live
  region says "Allowed: <title>" / "Denied: <title>". Normal and reduced
  motion preserve focus, selection, held/source-delay, refused/new-request,
  and unmount cleanup semantics. _Check:_ `responsive.mjs --only overview-leave`
  (Chromium and WebKit), with its old-owner omission; public unit tests
  `overview.test.tsx`.
- [ ] **The edge between the list and the peek resizes, as the window's
  other edges do**: at rest nothing of it shows — no fill, shadow, border or
  glow over the peek's hairline; near the pointer, or with the keyboard on
  it, the edge light glows along it. Dragged, the boundary follows the
  pointer (+120px moves it 120px); ← / → move it 16px; a double-click restores
  the even split. The list keeps at least 340px and the peek 320px, dragged
  or as the window narrows, and the dragged width holds while the window is
  open (the layer's state, not a stored preference). It is a separator
  (`aria-valuenow`/`min`/`max`) and a tab stop between the list and the
  peek. The peek's content — header, where it stands, the story and the
  reply — is one column centred in the peek at every width (its gaps either
  side equal ±2px, their edges shared ±1px). What the peek says scrolls above its reply pill,
  as a pane's transcript does above its composer (fading 20px at its edges):
  nothing is drawn behind the pill at any scroll, and the last line scrolls
  clear of it. _Check:_ `responsive.mjs --only
  overview-edge` (Chromium and WebKit); its place in the tab order, by hand.
- [ ] **One press answers one request** (_ADR 238 › What fills the content
  region_): a held ⌘↩ answers one; two queued ⌘↩ events stamped within
  250ms answer one, even as the first moves focus to the next request. The
  queued pair uses synthetic DOM events (`isTrusted: false`), records its actual
  timestamps and targets, and tests browser routing and the guard; the held-key
  check keeps real browser input coverage. A held ↩
  on a pane card's Allow Once answers one and sends nothing typed; a double
  click on Deny or Always Allow answers one. _Check:_ `focus.mjs`
  (`focus-answers-overview`, `focus-answers-card`); unit tests `walk.test.ts`,
  `overview.test.tsx`, `approval-request.test.tsx`, `composer.test.tsx`.
- [ ] **Escape, or anywhere else chosen, goes back to the panes** with the
  caret in the focused pane's composer — Escape straight after ⌘0 too, before
  the keyboard has landed on a row. _Check:_ `focus.mjs` (`focus-overview`,
  "⌘0 then Escape at once").
- [ ] **⌘R keeps the keyboard in the reply pill**: straight after ↓, with
  the words typed at once, every key lands in the pill of the row ↓ went to;
  sending, which moves the row from Needs you to Working and draws its pill
  anew, leaves the caret in that session's pill. _Check:_ `focus.mjs`
  (`focus-reply`, 1440 × 900 and 1000 × 700); unit test `overview.test.tsx`.
- [ ] **Focus the overview loses, it gives back**: whatever takes the focused
  element off the page, if only to put it back — its session changing group
  (`focus-regroup-row`), the peek beneath its row going (`-peek`), its row
  moved within its list (`focus-gives-back-moved`, which also holds the list
  where the person scrolled it: focus is given back on the next frame,
  without scrolling, and not over focus that landed elsewhere first; a moved
  element that is still on the page gets focus itself, so a reply being
  typed in a pill beneath a moved row keeps its caret: `-moved-pill`), the peek beside the list
  going as the window narrows (`-beside`), Show All once nothing is left out
  (`-show-all`), a count leaving the line (`responsive.mjs`,
  `overview-counts`) — the keyboard goes to that session's row, or else the
  current row, or the list, and the arrows walk it. Focus the person moved
  with a press elsewhere stays where they put it, even when the element goes
  in the same task (`focus-regroup-away`); a press that leaves focus where it
  is (the titlebar's drag strip) changes nothing (`focus-regroup-held`). _Check:_ `focus.mjs`, as named;
  unit test `overview.test.tsx`. Focus lost while the window is away
  (another app, or tabbed out of the page) is given back when it returns,
  unless the press that brings the window back puts it somewhere itself:
  _check:_ manual, in the app (a headless page never loses the window), and
  the unit test.
- [ ] **The sidebar marks what is shown**: Agents while the overview is, a
  channel or session only while the panes are. _Check:_ manual.
- [ ] **The header holds its place; only the list scrolls.** The title, its
  counts and the filter stay where they are while the list scrolls to its
  end; the list begins below the header and its top edge fades (a mask)
  rather than cutting a row. "Needs you" is shown only while something
  waits. _Check:_ `responsive.mjs --only overview-header` (Chromium and
  WebKit, at 1440 × 560 so the list must scroll); unit tests in
  `overview.test.tsx` for the empty section.
- [ ] **A count shows its group alone** (_ADR 238 › Showing one group
  alone_): the header's counts are toggle buttons reached from the filter by
  Tab (⌥Tab in WebKit); one chosen lists only its group and reads as pressed
  (`aria-pressed`), no count and not the header moving; chosen again, every
  group is back; with one chosen, the first Escape shows every group and the
  overview stays, the next leaves. With Needs you chosen and every request
  answered, "Nothing needs you" heads the list with the session looked at
  still listed and chosen beneath, under its new heading; once it rests, the
  filter keeping it out, no count counts it; its count, let go at nought from
  the keyboard, gives the keyboard to the list, whose arrows walk it — never
  `<body>`. _Check:_ `responsive.mjs --only overview-counts` (Chromium and
  WebKit, both layouts, its default); unit tests
  `overview.test.tsx` (the counts show one group alone, the footer's count and
  Show All under a group), `commands.test.ts` (the group's table),
  `agents-glance.test.ts`.
- [ ] **The peek tells the turn's story** (_ADR 238 › The peek tells the
  turn's story_): the source's line of what is going on (one line), then the
  person's latest message, "Earlier in this turn · Open" where the turn holds
  more than the peek draws (at most 24 parts), the agent's latest work in
  order, and the request last, each below the one before; a long turn scrolls
  within the peek while the list's header holds; beneath a row (760px wide),
  the story is reached from its row by Tab (⌥Tab in WebKit), keeps its
  scrollbar, and scrolls on PageDown and End without the list moving the
  keyboard away. _Check:_ `responsive.mjs --only overview-story` (Chromium
  and WebKit, both layouts, its default; 1440 × 640, then 760 × 640); unit
  tests `overview.test.tsx` (the peek tells the turn's story; the peek's
  story is bounded), `peek.test.ts`,
  `in-memory-source.test.ts` (the source's line at each beat).


## Widgets

_ADR 326_ ([`docs/adr/done/326-widgets.md`](../../docs/adr/done/326-widgets.md)):
a plugin's view is drawn inline in a message, in a pane of its own, or in the
window over the panes; Escape and focus as its _Focus_ and _Escape_ say. The
scripts drive the sample plugin the sample workspace registers
(`src/desktop/widgets/fixture/`), on its session "Widget hosts, every state".

- [ ] **A card opens its widget in a pane beside its conversation, and the
  window over the panes, and Escape goes back** — the pane's rect beside the
  conversation's, the caret in its body; the window inside the chat area and
  clear of the session list, the caret in its body; its body and header fully
  opaque and its root/header at unit transform from the first mounted frame,
  because it covers the main chat; the panes as they were
  after Escape, the caret back in the focused pane's body.
  _Check:_ `widgets.mjs --only card-pane-window`.
- [ ] **The window is left as the overview is, with ⌘W its own**: its close,
  ⌘W (no pane beneath it closes), a session chosen; ⌘0 from it opens the
  overview. _ADR 326 › Places._ _Check:_ `widgets.mjs --only window-left`.
- [ ] **Escape steps back in a view first, and the caret stays in the widget**:
  a detail open in a pane or the window is closed by Escape, the caret falls
  back to the widget's body, a second detail is still Escape's to close; in
  the window the next Escape goes back to the panes; a widget pane is never
  closed by Escape. _Check:_ `widgets.mjs --only escape-steps`.
- [ ] **What a host cannot draw, it says**: the off and missing cards say the
  hosts' own lines (`hostDraws`, read from the page). _ADR 326 › What a host
  draws._ _Check:_ `widgets.mjs --only off-missing` (`--mode dev`); every row
  of the table, in every place, is `host-table.test.ts` and `hosts.test.tsx`.
- [ ] **⌘1–4 and ⌘W landing on a widget pane put the caret in its body.**
  _ADR 238 › Focus follows the focused pane_, as ADR 326 amends it.
  _Check:_ `widgets.mjs --only focus-keys`.
- [ ] **Escape from a session row leaves the window; from the search, it
  clears the query first and leaves on the next.** _Check:_ `widgets.mjs
  --only escape-row-search` (the search: three columns, which has the list).
- [ ] **With the edge peek shown over the window, Escape dismisses the peek
  and nothing more.** _Check:_ `widgets.mjs --only escape-peek`, run ten times
  in each engine and layout when the peek's Escape changes (it failed 2 of 8
  in WebKit while the peek's listener came an effect after its commit). In
  Settings, a dialog itself, Escape dismisses its own peek and Settings stays:
  `widgets.mjs --only escape-peek-settings`.
- [ ] **Beside the window the sidebar keeps the channel and the focused
  session marked, and the Agents entry unmarked**, in both layouts.
  _Check:_ `widgets.mjs --only sidebar-marks`.
- [ ] **A session row carried over the window finds no target**: no zone said,
  no placeholder, and the release changes neither the panes nor the window.
  _ADR 238 › Drag and drop_, the no-target row. _Check:_ `widgets.mjs --only
  drag-over-window`.
- [ ] **A view is told its place's size**, as the place changes: a card grown
  by its content alone and by a font, a pane after a window resize and a
  split, the window after a resize — the size in its host context in whole
  pixels, within a pixel of the place's content box. _ADR 326 › The
  contract_ (the host context). _Check:_ `widgets.mjs --only host-size`; it
  fails when the observer stops after its first report.
- [ ] **A widget pane's chrome fits a narrow pane in a short window**, and the
  window's at the same size: no header overflow, the trail and close inside
  it, the body with room. _Check:_ `widgets.mjs --only narrow-short` (1000 ×
  560, three panes).

## Subagents

The read-only sample panel (ADR 329, #330, #331, #332): a conversation's
widget opens a list of the agents it put to work, and one child's
conversation. The sample fills the retry-budget session only. No composer.
The same conversation's pane header shows those agents when the preview is
on.

- [ ] **The card opens a pane beside the conversation**, the list in activity
  order with a closed child called closed, and a child opens on its
  transcript. Focus stays in the panel. _Check:_ `subagents.mjs --only panel`.
- [ ] **The transcript follows a new line while it is at the end, and stays
  put when scrolled up.** _Check:_ `subagents.mjs --only panel`.
- [ ] **Escape steps back to the list.** In a pane a second Escape changes
  nothing more; in the window it returns to the panes. _Check:_
  `subagents.mjs --only panel`.
- [ ] **The list fits a narrow pane in a short window.** _Check:_
  `subagents.mjs --only narrow` (1000 × 560).
- [ ] **The conversation's header shows its subagents, the busiest first.**
  Nothing is drawn for a conversation without them, or with the preview off
  (`subagent-stack.test.tsx`). A click opens the panel beside the
  conversation. At a narrow width the stack, the header's title and the
  pane's menu sit in the header without it overflowing and without
  overlapping, and the title keeps a positive width inside the header.
  A production Chromium run fails when the open's longest frame exceeds
  50 ms. _Check:_ `subagents.mjs --only narrow` (1000 × 560).
- [ ] **The card's Open is measured the same way.** A production Chromium
  run at the same size fails when that click's longest frame exceeds
  50 ms. _Check:_ `subagents.mjs --only card-open` (1000 × 560).

## MCP Apps

_ADR 344_ ([`docs/adr/done/344-mcp-ui.md`](../../docs/adr/done/344-mcp-ui.md)),
#349: an MCP server's app is drawn behind a sandbox proxy on another origin,
inline, in a pane (its fullscreen) and in the window, and spoken to over the
`ui/*` bridge. The script drives the fixture app the sample workspace
registers (`src/desktop/widgets/app/fixture/`), on its session "An MCP App,
in its sandbox". Every row of the bridge's design table is a jsdom test
(`app/application/bridge.test.ts`, `app/ui/app-view.test.tsx`).

- [ ] **Actions in a discovered app wait for current finite host motion.**
  A same-origin nested-frame geometry control moves its transformed ancestor
  during mousemove; discovery remains available before motion finishes, and
  settlement on those same handles makes Close receive exactly one trusted
  click. This checks the demonstrated automation gap in #906, not the unproved
  cause of its original teardown failure. _Check:_ `mcp-apps.mjs --only moving-frame`.
- [ ] **It renders in all three places, told its mode and its call** — the
  card's app live, told `inline`, its call's input and result, its frame as
  tall as the app says; its fullscreen request opens a pane beside the
  conversation (answered `inline`: the card stays), whose app is told
  `fullscreen` and fills its body; Open in Window shows it in the window,
  filling it. _Check:_ `mcp-apps.mjs --only inline,pane,window`.
- [ ] **`tools/call` is answered for an allowed tool and refused for a hidden
  one**, with the gateway's reason. _Check:_ `mcp-apps.mjs --only tools-call`.
- [ ] **An app's message lands in its conversation as the person's, saying
  which app wrote it** (#390, rows D1, D4, D17 of the desktop's table in
  [`docs/design/mcp-app-calls.md`](../../docs/design/mcp-app-calls.md#an-app-in-its-conversation-the-desktop-390)):
  `ui/message` is answered `{}`, and the transcript gains exactly one message
  of the person's with the app's words, labelled "Sent by show_fixture, from
  nessa-fixture" — wrapped, never cut, so with no `title`; each name in its
  own `<bdi>`; `data-message-app` `server/tool` — above its bubble, over the
  bubble's right edge (within 6 px) and inside the column; the person's own
  messages carry none; another while the sample's reply runs is refused
  (`isError: true`) and adds nothing; `ui/update-model-context` is refused
  "The sample has no model to give context to" (gate 7). The gateway's own
  path is `app-messages.test.ts`, `bridge.test.ts` and `app-review.mjs
  --only message`. _Check:_ `mcp-apps.mjs --only message`.
- [ ] **A request to an origin the app did not declare is blocked by its CSP,
  and the host says so** above the app. _Check:_ `mcp-apps.mjs --only csp`.
- [ ] **The app is on an opaque origin**: no parent or top document, no
  storage, no cookie; the proxy on another origin than the window.
  _Check:_ `mcp-apps.mjs --only isolation`.
- [ ] **An app cannot leave its frame for a page without its policy**: sent
  away by script, by `<meta refresh>`, or after rewriting its document
  (`document.open()`, which erases its reporter), its frame's navigation is
  refused by the proxy's policy (nothing reaches the other site). Either the
  app stays, live, in its own document (WebKit refuses the navigation before
  it leaves), or the frame is taken off the page and the host says it cannot
  show the app: within 5 s with its reporter in place (the reporter's word on
  `pagehide`), and after a rewrite once its frame's load goes unanswered.
  Forged proxy messages change nothing, and a forged report puts none of the
  app's words in the host's chrome. _#349 design, L32 and Sandbox._
  _Check:_ `mcp-apps.mjs --only escape-navigate,escape-refresh,escape-rewrite,forge`.
- [ ] **Every way the app's document is replaced is its departure, and
  nothing else is**: the real proxy, in the host's own frame, handed what the
  host's own builder writes with the host's deadline — another document in
  the frame (a reload, before or after the first load; a rewrite, closed or
  then sent away; a navigation or `about:blank`, even after the app
  dispatched checks of its own making, or had a frame of its own send them
  with `source` patched; a document with no reporter, one answering without
  the token, or one whose own frame answers with it) — reports the app gone
  once and relays nothing of it after (a reloaded document's own scripts
  included); an app left alone (which never hears the check, even having
  patched the event APIs), its links to a fragment of any kind (`<a>`,
  `<area>`, SVG, in a shadow root, `target="_self"`, a spaced `href`) and its
  moves to one by script (WebKit loads the frame for these), a first load
  held by an image the fixture leaves unanswered, going back across a move to
  a fragment (on a page of its own, where an answer to an earlier check,
  coming after a later load, ends no wait), an app forging departures, and a
  third party forging them and the check's answers at every proxy and app
  frame, are not; a host reply with no method of its own, after the app has
  defined `method` and `params` on `Object.prototype`, is not a check, and
  that app's departure is still reported; a `javascript:` rewrite does not
  run in Chromium, and in WebKit it runs and the unanswered load is the
  departure; a deadline no timer can wait loads
  nothing. _#349 design, L32; #388._ _Check:_ `mcp-apps.mjs --only
  departures,departures-back` (dev server: it imports the host's builder; it
  waits past the initialize deadline).
- [ ] **The test server's chart app goes live** (#423): handed to the real
  proxy as the host writes a document, it says `ui/initialize`, reaches
  live, and `#chart` shows the series from the tool result it was sent
  (`alpha 10, beta 20`), not a static page. _Check:_ `mcp-apps.mjs --only chart`
  (dev server: it imports the host's builder).
- [ ] **It is torn down on close**: a pane's close takes its proxy and app
  documents with it; an app asking to go is sent `ui/resource-teardown`, and
  its pane closes only once it answers. _Check:_ `mcp-apps.mjs --only teardown`.
- [ ] **In the desktop app** (by hand, `pnpm app`): the proxy loads from the
  `nessa-sandbox` scheme and the fixture app renders in each place.
  _ADR 344._ Not scriptable here: the scripts drive the browser build.
- [ ] **A real server's app works over a real gateway** (#384): in a browser
  preview signed in to a gateway of its own, the test MCP server's review app
  renders inline in the conversation where the agent called `review_rows`, in
  the sandbox; its calls to tools hidden from apps are refused, and it shows
  so; its destructive call waits on a review in the conversation's
  permissions, its origin the app, shown in the window, and the app shows
  the answer to the review's Allow and to its Deny (the card does not offer
  Always Allow); closing the pane of a mount with a
  call waiting withdraws the review, the window's card for it goes once it is
  withdrawn, and the inline mount stays. _#349
  design, L14 and L24._ _Check:_ `mcp-apps-gateway.mjs` (needs the gateway
  built and the agent, `--agent claude|codex`, signed in on the machine). The
  refusal of the hidden tool that declares no UI depends on #412.
- [ ] **An app's message and context, with no model key** (#550): with `--scripted`, the review app's message control opens a review; Allow lands the message in the transcript labelled as that app's, and its context control answers success. Chromium and WebKit share the conversation and send the same text, so each engine records execution ids before the click and requires exactly one new send, and the transcript row that was not already shown. The scripted agent replays one recorded turn, then answers a later prompt with text and no tool call, so the check needs no Claude key and no user model key. _Check:_ `mcp-apps-gateway.mjs --scripted` (`message`, `context`). `messagesArrived` in `gateway-view.mjs`.
  Its message input reuses the release check's trusted-pointer entry before
  one real click. A missing fixture pending state is reported as missing input;
  an Allow is named only if the check actually clicked it. See the
  [sampling order](../../docs/design/ui-workspace-load.md#verification-sampling-order).
- [ ] **One tool call is drawn once** (#418): a harness reports one call as an
  announcement and then updates under its id (Codex three frames, or two
  in the recorded `review_rows` turn; Claude four), and the window draws it as one transcript step and one inline app
  frame, so the app mounts once. _#418 design._ _Check:_
  `mcp-apps-gateway.mjs --agent codex --scripted` and `--agent claude
  --scripted`, `renders`: its step and frame counts (no model, no sign-in: the
  scripted agent replays the recorded frames under the real gateway).

## Composer and approval card

- [ ] **An agent's command is drawn in the order it runs.** A bidi control in
  the argument (U+202A–U+202E, U+2066–U+2069, U+200E, U+200F, U+061C, and the
  line and paragraph separators U+0085, U+2028, and U+2029) is shown
  as its `\u` escape, so an override cannot present another address; the shown
  JSON parses to the argument that runs. A command that starts with a
  right-to-left letter keeps a left-to-right base, so the tool stays left of
  its argument. The same draw is the card, the overview row, and the peek
  (`shownCommand` in `said.tsx`, `dir="ltr"` on the command's isolate). _#553._
  _Check:_ `command-order.mjs`.
- [ ] **The approval card arranges itself by its own width** at 280, 340,
  420, 600 and 900 px, with a short and a very long command: no word of the
  command broken, no button label wrapped, nothing overflowing.
  _ADR 238 › Decision_ (`ui/`, `approval-request.tsx`).
  _Check:_ `responsive.mjs --only approval-card --shots <dir>`, then look at the shots.
- [ ] **An agent's command is always shown, with what it names above it.**
  The card's row names the sites the command names (each by a globe: no
  site is asked for its icon before the person answers) and the programs it
  names, the shell's own words (`if`, `then`, `do`, `fi`) left out (no row when it would only repeat the command's first
  word); under it the script, one step per line where the card
  follows its quotes and exactly as sent where it does not, then the tool it
  calls, the tool's other input, and Copy Command. A script over six lines
  shows its first four and Show All N Lines; the next approval starts folded.
  A title that only repeats the tool's name is not shown as a reason
  (`approval-card.tsx`, `approval-gist.ts`).
  _Check:_ `transcript.test.tsx`, `approval-gist.test.ts`; `responsive.mjs
  --only approval-card` unfolds a long script before it measures.
- [ ] **An armed review counts down, then the card is the automatic notice**
  (#141): the deadline ring shows the seconds left ("Denies in Ns" in its last
  ten) and is named "If nobody answers within N seconds, this is denied
  automatically.", N goes down while the card is up, and at expiry the
  card is gone and the transcript says "Denied automatically: …". The
  fixture's gateway clock runs a minute ahead of the page's, and N starts no
  higher than the wait: the card counts down from the gateway's `serverNowMs`,
  not the window's clock (ADR 141 › What a person sees). Chromium
  and WebKit. The fixture is `fixtures/approval-deadline/`, dev server only.
  _Check:_ `approval-deadline.mjs`. The card's own widths stay
  `responsive.mjs --only approval-card` at 280 and 340 px.
- [ ] **A clock snapshot is stable across React's consecutive reads even when
  injected time crosses a label bucket**, then one published interval tick
  updates the reader and unmount clears the last timer. The page uses only the
  public `ClockProvider`/`useNow` API; any console or page error fails in both
  Chromium and WebKit.
  _Check:_ `clock-snapshots.mjs` (`fixtures/clock-snapshots/`, dev server).

  | Case | Setup and action | Required browser evidence | Enforcement evidence |
  | --- | --- | --- | --- |
  | CL1 — consecutive reads | Inject `now()` as 999 for its first read, then 1000; render the actual hook in StrictMode. | The published-snapshot hook stays error-free while the old time-derived getter emits React's uncached-snapshot warning in both engines. | Replace the cached getter with `Math.floor(clock.now() / interval)`; the case fails on the warning. |
  | CL2 — published tick | Hold the active 1000 ms timer callback, record committed `999` and its render count, advance to 1000, then invoke that callback. | One tick commits `1000` and increases the DOM render count beyond the recorded baseline. | Remove the listener notification from the callback; the case fails before any new render commits. |
  | CL3 — last reader cleanup | Unmount the last reader after CL2's tick. | Every StrictMode interval start is matched by a stop; the stopped timer cannot publish again. | CL2 asserts the last-reader timer cleanup counts. |
- [ ] **An app's review is drawn, and names the app, not the agent**, in the
  window over a fake gateway (`fixtures/app-review/`, dev server only): at
  rest, a conversation whose turn has ended is not read again; when its app
  calls a destructive tool, the fake opens the review only once the
  conversation has been read twice since the call (nothing in the list row
  moves), and the card is drawn within 8 s, its head "The <server> app wants
  to run <tool>" (`data-origin="app"`); the card offers only the review's
  Allow and Deny (no Always Allow); Allow sends one answer, Allow for
  that review, the card goes, the app's call comes back ok, and the reads
  stop. At the same widths as the card above, with the tool's name short and
  as one word as long as the gateway allows (`maxMcpNameBytes`), the head
  stays inside the card; its row in the Agents overview is named "<title>. The
  <server> app wants to run <tool> <arguments>.", the server and the command
  each between FSI and PDI. _#436_ (`appCall` in
  `gateway-source.ts`, the `callTool` routing in `dependencies.ts`;
  `approvalHead` and `approvalAsker` in `approval-request.tsx`).
  Other conversation, turn or review answers leave the original simulated
  request waiting; its own Deny settles tool and message requests. Answer
  records and callbacks use the SDK method's parameter/result types
  (`fake-gateway.test.ts`, fixture typecheck).
  _Check:_ `app-review.mjs --shots <dir>` (including `target`).
- [ ] **An app's message is reviewed as a message, and lands labelled** (#390,
  rows D9, D17, D18, D19), in the same fixture: a context the app gives is
  taken at once, draws no card and starts no read; the app's message is read
  each round until its review is drawn, its head "The <server> app wants to
  send a message as you" (`data-ask="message"`, `data-origin="app"`) and its
  command the app's tool and `{"text":…}`, the message whole; Allow Once sends
  one answer, Allow for that review, the app is answered ok, the message lands
  labelled "Sent by <tool>, from <server>" over its bubble's right edge
  (within 6 px), and the reads stop; a second message denied is refused and
  lands nothing. With a message as long as the gateway takes
  (`maxMcpMessageBytes`), of words and of one unbroken word, at 280, 340,
  420, 600 and 900 px, the head stays inside the card, the card does not
  overflow, and Allow Once stays reachable (scrolled to, it is what the page
  hits at its centre). The overview row is named "<title>. The <server> app
  wants to send a message as you.", the server's name between FSI and PDI,
  and its tooltips say "Don’t send it" and "Send it once". The label, with
  the app's names short and each as long as the gateway allows, is whole in
  a message column of 280–900 px and in an 800×480 window: no ellipsis, no
  title, no glyph outside it nor past its column, wrapped onto more lines at
  280 px, each name in its own `<bdi>`. With names carrying bidi controls —
  a stray PDI then an embedding (`a`, PDI, RLO, `b` and `c`, PDI, PDI, RLE,
  `d`), and an override (`evil`, RLO, `gnp.exe`) — the card's head, the
  gateway's title on the card (each name in its own `<bdi>`), the command's
  tool name (the message's words after it kept), the overview row's
  accessible name and its command, and the landed label show each control
  as U+FFFD, each name isolated, and every character drawn in reading order
  (`shownName` in `said.tsx`, E2-1). (`ask` from the gateway's
  `ReviewAsk`; `approvalHead` and `approvalRequest` in
  `approval-request.tsx`; the `sendMessage` routing in `dependencies.ts`.)
  The real gateway's message path in a browser is #550.
  _Check:_ `app-review.mjs --only message,message-card,message-overview,message-label,message-bidi --shots <dir>`.
- [ ] **An established conversation changes model through its existing picker,
  within the same provider.** The source-confirmed model remains shown while
  the correlated control and authoritative source recovery settle;
  refusal/recovery is explicit. An unsent draft,
  original review and Stop survive the control. No automatic native retry.
  _Check:_ model command/adapter tests; `conversation-stress.mjs`'s actual-window
  model step (default `--modelSwitches 1`, raised `--modelSwitches 6`), both
  harness profiles in focused runs; the next native prompt reports its model.
  Positive model steps follow the existing non-model/preset steps and confirm
  Ask through the real access shield before creating their original review.
  The existing chaos model action additionally races controls and checks no
  pending model change remains; client-only chaos does not replace window proof.
  Window stress opens each known conversation by its exact existing row ID;
  equal titles across engines or conversations never choose the send target.
- [ ] **The model is shown once, in the composer** — not in the pane header
  or the transcript heading. _Check:_ manual (and in shots from `responsive.mjs`).
- [ ] **Composer controls never overlap**, down to the compact form.
  _Check:_ `responsive.mjs --only composer-chips`.
- [ ] **The thinking control changes level without moving anything**
  (_ADR 238 › The thinking control_): from the keyboard it opens with the keys
  on the slider; the arrows, Home and End change the level, the chip's name
  follows, and only Ultra — past Max, where the model has it — is marked
  apart; dragged, the knob stays on the pointer (± 2 px), the level is the
  nearest, and let go the knob settles on its level (± 1 px); as ⌘B and ⌥⌘S
  move its chip, the popover stays on it (its edge on the chip's ± 1 px, 10 px
  above it ± 1 px); each model the check names offers exactly the levels the
  SDK catalogue publishes for it (_ADR 302_), Fast only where the catalogue
  records it (Claude Opus 5 has it, Claude Sonnet 5 not), no levels at all
  where none is recorded (Claude Haiku 4.5, its chip disabled), and Ultra only
  past a published Max — which no catalogue model has today; None carried from
  GPT-5.6 Sol to Claude Sonnet 5 shows as its least level, and that level
  chosen there is still it back on GPT-5.6 Sol; in every frame of
  every change, of the drag, and of turning Fast on, the composer, its
  controls and the popover hold their place (± 0.5 px); a level's change
  animates transform and opacity only; Escape, and Tab past its end, close it
  onto its chip; with the system's reduced motion nothing animates and the
  words that were shown are not drawn.
  _Check:_ `responsive.mjs --only thinking-control` (`--shots <dir>` saves it
  opened and at the utmost level); its look by eye in each theme, light and
  dark, with and without Fast (Claude Opus 5 and Claude Sonnet 5),
  against the menus and chips beside it.
- [ ] **Send from a new session arrives in its transcript.** _Check:_ `smoke.mjs` (`send`).
- [ ] **A new session's home takes its pane's shape.** In a pane under 640px
  either way its composer docks at the foot as the conversation's beside it
  (same distance from the foot, inset and height, ± 1.5 px), with the
  greeting on and turned off; the header stays as a band from the top of the
  pane (72–150px of picture or scene) with its Customize control — in the
  shortest window too, below the window's home's 520px cutoff — and the
  greeting sits under it at the pane's left at the conversation title's size,
  weight and tracking (issue #320); a larger pane keeps the scene and the
  card. A home appearing,
  large or small, plays no settling; each crossing plays one, by opacity and
  transform alone, and with less motion the composer is there on the next
  frame. Across each crossing the same field keeps its draft, the model
  chosen and an open page, and across a resize the caret keeps its focus and
  position. _ADR 238 › A new session's home takes its pane's shape_.
  _Check:_ `responsive.mjs --only home-shape --shots <dir>`, then look at the
  shots beside the conversation pane.
- [ ] **First-message handoff has one composer.** Sending from a new-session
  home replaces it without overlapping composer instances; the reply receives
  the caret and keeps the next draft. The composer glides into its dock and
  the sent words rise into their bubble; reduced motion performs neither flight.
  The native home field's focusout precedes that Enter's pane DOM mutations
  while the original home and field are still connected (ADR 238's handoff order).
  _Check:_ `focus.mjs` (`focus-home-handoff`, `focus-home-handoff-reduced`).
- [ ] **Focus on Customize survives the home becoming small.** Customize
  focused in a new session's home, the window shortened until the home takes
  a small pane's shape: the header stays, and focus stays on Customize.
  _Check:_ `focus.mjs` (`focus-home-scene`).
- [ ] **Any pane's menu chooses the header picture.** From a conversation
  pane's "…" menu, Choose Header Picture… opens the file dialog and the
  picture chosen shows in the panes' bands; a new session's home draws it from
  its pane's top edge, behind the title row — except in the window's corner
  with nothing beside it, where nothing of it is under the window's controls;
  Use Night Scene, offered only with a picture, takes it back; a file that is
  not an image is refused with why, in that pane's header (issue #320).
  _Check:_ `responsive.mjs --only header-picture`, and `safe-area.mjs`.

## Settings

_ADR 238 › Decision_ (Settings is a typed catalogue; modal; its sidebar folds below a page's 420px).

### Linked devices

The scripted socket also answers the real window's automatic `workspace.publish`
with its per-connection revision receipt; unknown methods remain failures.

#462, over the owner pairing routes. The code, fingerprint and orbs are the
UI kit's; this window wires them to `pairing.*` and owns the sentences. The
phone's scanner is not this screen.

- [ ] **With no gateway, Linked devices is pending.** _Check:_ unit test
  `linked-devices-tab.test.tsx`; `linked-devices.mjs` (`pending`).
- [ ] **Linking off, refused, pair, approve, revoke, and an expired code**
  are said in plain words. The switch does not pretend to write config.
  A wire code is not on the page. The code, both orbs and the actions sit
  inside the settings panel, and Cancel takes focus once it can be used.
  _Check:_ `linked-devices.mjs` (Chromium and WebKit). Rows L1–L21:
  `linked-devices.test.ts`, `linked-devices-gateway.test.ts`,
  `device-key.test.ts`.

- [ ] **Settings opens and leaves**; the window under it is inert while open.
  _Check:_ `smoke.mjs` (`settings`); inertness by hand (click under it).
- [ ] **Its sidebar folds for room below a 420px page.** _Check:_
  `responsive.mjs --only settings-fold`.
- [ ] **Pending settings say "Not available yet" once, and their controls
  are disabled** — nothing looks as if it works when it does not. A group
  whose settings are all pending says it once under its card, and each
  disabled control is described by that note (`aria-describedby`); a pending
  row in a group with others says it on its own row. _Check:_ unit tests
  `settings-view.test.tsx`, `settings-controls.test.tsx`; by eye.
- [ ] **A setting for one layout says so elsewhere**: "Show session list" and
  "Keep running sessions at the top" work in three columns; in sessions in
  the sidebar and classic their switches are disabled and say "Three columns
  only". _Check:_ unit test `settings-view.test.tsx` (per layout).
- [ ] **Segmented choices are one Tab stop each option, for now**: they are
  the kit's `SegmentedControl` (a group of pressed buttons, no arrow keys);
  one Tab stop with arrows waits on its radio-group mode (nessalabs/nessa_ui#120).
  _Check:_ by hand.
- [ ] **Advanced › Experimental is the home of previews**: Advanced sits just
  before About with its flask in both icon families; one line under its
  title says what it is for, and with one tab there is no tab strip; it
  offers each preview as one switch — the subagents preview (on unless
  turned off) and the side rail (off until turned on); General has no Experimental tab; search finds it as "advanced",
  "experimental", "labs" or "preview". _Check:_ `smoke.mjs` (`settings`);
  unit tests `settings-view.test.tsx`, `settings-catalogue.test.ts`,
  `icon-provider.test.tsx`; the flask by eye in both families.
- [ ] **A page's title stands clear of the window's controls**: at 1440 × 900,
  1000 × 700, 760 × 700 and 600 × 700 (sidebar folded) every category's
  title starts at least 16px below the titlebar row; at 1000 × 700 every row
  of General, Appearance, Workspace, Models and Privacy & Permissions (their
  first tabs) is above the fold. _ADR 238 › The titlebar's safe area._
  _Check:_ `settings-page.mjs --only masthead`.
- [ ] **No word shows twice at once**: no tab in a strip is named as its
  category is (General's first tab is Startup); no group's title on screen
  repeats its page's or its tab's name (Workspace › Keyboard's groups are
  Window and Panes and sessions; Linked devices lists Your devices); and a
  card named as its tab (Agents, Providers, Layout, Linked devices) keeps its
  title for assistive technology only. _Check:_ unit tests
  `settings-catalogue.test.ts`, `settings-view.test.tsx` (every page without
  a gateway); Linked devices with a gateway by eye.
- [ ] **Scrolled, the page's bar names it**: once the title has scrolled under
  the bar, the bar shows the page's name small — starting with the column,
  or after the window's controls with the sidebar folded — and it goes when
  scrolled back. _Check:_ `settings-page.mjs --only condense` (1000 × 700,
  600 × 700); `safe-area.mjs --only settings,settings-sidebar-return`.
- [ ] **A page arrives quickly, and at once under less motion**: a category or
  tab change runs at most 300ms of motion, on the masthead and the page only,
  and none under Reduce motion. _Check:_ `settings-page.mjs --only arrive`.
- [ ] **Search lands on the setting**: Enter takes the first result's
  category and tab, brings the setting to the middle of the view and marks it
  for a moment; ⌘F goes to the search field, showing a hidden sidebar first.
  _Check:_ `settings-page.mjs --only search`.
- [ ] **Keyboard focus shows the window's focus ring** on Settings' tabs and
  controls (not a fill). _Check:_ `settings-page.mjs --only focus-ring`
  (WebKit walks with ⌥Tab, as Safari does).
- [ ] **Review notifications ask from Allow.** Access shows the row and
  does not prompt before that button is pressed. _Check:_
  `settings-page.mjs --only review-notices`.
- [ ] **Remembered rules rest without a gateway.** Access shows one
  remembered-rules row, disabled, and says it is not available in this
  window. _Check:_ `settings-page.mjs --only remembered-rules`; unit test
  `permission-rule-rows.test.tsx`.
- [ ] **Claude and Codex native hooks have their own tab and rest without a gateway.**
  Access keeps the common controls above the fold; Native hooks shows
  both rows disabled, with no switch, and says hooks can approve, deny, or
  change a tool call and its result without Nessa asking, and can run
  commands when the agent starts. Search for either hook lands on its row
  in view on Native hooks. OpenCode has no row. With a gateway
  and Claude's choice on, that row's switch is on and says to turn it on
  only for projects you trust, and that the choice applies the next time
  each conversation's agent starts. Claude on but missing from the catalog
  says it isn't set up. A refused change says it could not change. _Check:_
  `settings-page.mjs --only native-hooks,native-hooks-on,native-hooks-unset,native-hooks-failed`; unit tests
  `native-hooks-rows.test.tsx`, `native-hooks-gateway.test.ts`.

### Integrations: the gateway's MCP servers

#391 PR 3's design (the issue's comments of its state table, rows U1–U31,
U32–U43 from its review, U44–U50 for a list too large to show, and S1–S8,
G1–G9 and F8–F12 for the secret field and names stored more than once):
Settings › Connections › Integrations manages the gateway's stored MCP
servers over `client.mcpServers`. Every row is a test of
`settings/model/mcp-servers.test.ts`, `settings/ui/integrations-tab.test.tsx`
or the script below. Gate 13: the window shows no limit the protocol does not
publish, and refuses nothing the gateway would judge.

- [ ] **With no gateway, Integrations is pending** — the sample preview,
  whose workspace is the in-memory one (U1); the app window and the
  `?gateway` preview have one. _Check:_ unit test `integrations-tab.test.tsx`.
- [ ] **Each write is the gateway's, and the list is what is shown** (gate 16):
  added with a variable, one row, its switch on, "1 variable", and the
  variable's value nowhere in the page — markup or field — after the save
  (U7, U8, U31); inspected, seen running — the test server slow to start
  (`--initialize-delay-ms`), so the running state is read, not raced — with
  Inspect and Close resting, then `show_chart` badged UI and `app_delete_row`
  destructive, complete (U22, U23); the switch
  rests while its save is in flight and shows the new list's state (U12);
  renamed, one row, its variable kept, the stored value "Stored value kept"
  (U10, U11); given another command, the gateway refusing a kept value under
  it, the value asked for again, why said, and Save held until it is typed,
  then saved with the row showing the new command (U33); a conflict made by
  another writer first is refused, said, the list reloaded, what was typed
  kept and what was not refilled from the reload (U15, U42); removal asked first, nothing
  sent until confirmed, then the row gone (U13, U14), and an inspection
  running as its server is removed says the server is gone; Inspect rests
  while a write is in flight (U21). Each write is exactly
  one `mcpServers` request and one list after it. _Check:_
  `mcp-servers-gateway.mjs --only empty,add,inspect,toggle,rename,relaunch,conflict,remove`
  (needs the gateway built and an agent's harness installed).
- [ ] **Focus follows what opens and closes** — Add and Edit put it on the
  form's first field; Remove on the confirm's Cancel, which its sentence
  describes; Inspect on the panel's heading; Cancel, Escape, Save and Close
  put it back on the row's button that opened the part, or on Add (a save's
  once the list after it is read). Escape closes the form and the confirm,
  and never Settings. _Check:_ `mcp-servers-gateway.mjs --only focus`
  (`document.activeElement` after each); unit tests `integrations-tab.test.tsx`
  (`focus`).
- [ ] **Arguments and values are kept as typed** — one field per argument,
  so an empty argument and one with a line break are each one argument;
  Enter adds the next argument after it, Shift+Enter is a line break, marked
  under its field; removing an argument or variable focuses the next one, or
  Add (F11, F12). A variable's value is a password field, so a typed value is
  in no accessibility tree; a paste with line breaks is held and never drawn,
  "Pasted value: N lines", a trailing line break pointed out and trimmed only
  when asked, Clear to drop it (S3–S6). A stored value not edited is kept; edited
  to empty, it is cleared, said so, with "Keep stored value" (S1, S2, Codex
  P1). _Check:_ `mcp-servers-gateway.mjs --only secret` (the accessibility
  tree over CDP in Chromium, the ARIA snapshot in WebKit); unit tests
  `mcp-servers.test.ts`, `integrations-tab.test.tsx`.
- [ ] **A name stored more than once is one read-only group** — config.json
  edited to store three servers under one name: "3 servers share this name",
  each read-only with no Edit, Inspect or switch, and one action "Remove the
  first server named …", asked first in those words; each removal takes the
  first stored, focus staying on the group while it lasts, then on the row
  left (G3–G6). _Check:_ `mcp-servers-gateway.mjs --only duplicate-names`;
  unit tests (G1–G9).
- [ ] **What the gateway says is read out** — the notices, each field's
  problem and the inspection's status are live regions drawn before their
  text arrives, a field naming its problem with `aria-describedby`; no
  variable value is in the markup, after a refused save too (U9, U31).
  _Check:_ unit tests `integrations-tab.test.tsx`.
- [ ] **A failed list's notice goes once a list succeeds** — config.json made
  unreadable and the connection dropped, the failure is said; restored and
  dropped again, the list is shown with no notice. A write's "not confirmed"
  stays through the lists after it until the next action. _Check:_
  `mcp-servers-gateway.mjs --only reconnect`; unit tests `mcp-servers.test.ts`.
- [ ] **A list too large to show offers a remove by name** — config.json
  edited by hand to 15 servers with long arguments, under its own 64 KiB, whose
  list will not fit one frame (the gateway refuses it
  `mcp_servers_config_too_large` with a revision): the panel says "The server
  list is too large to show. Removing a server fixes it: enter its name.", the
  name field described by it, no row, no Add, and Remove resting until a name
  is typed; a server removed by name is asked first, nothing sent until
  confirmed, then one remove and one list, and the list shown again without it
  (U44, U45). A save that would push the list past the bound, the file still
  under its own, is refused: the form stays open with "This would make the
  server list too large; remove a server or shorten its arguments.", what was
  typed kept, no notice, nothing listed again and config.json unchanged (U48).
  _Check:_ `mcp-servers-gateway.mjs --only too-large,save-too-large`; unit
  tests `mcp-servers.test.ts`, `integrations-tab.test.tsx` (U46, U47, U49, U50).
- [ ] **A credential without `credential.manage` sees "Only an administrator
  can manage MCP servers", no control, and sends no `mcpServers` request**
  (U2). _Check:_ `mcp-servers-gateway.mjs --only non-admin`.
- [ ] **A remote row shows its URL and consent state, and Authorize and Revoke
  name that server's id and the list revision.** Waiting for consent shows the
  consent URL as a link and does not put a token on the page. _Check:_
  `integrations-tab.test.tsx` ("a remote row shows its consent state") and
  `mcp-servers.test.ts` ("remote authorization"). A consenting remote account
  is an owner-run check and is not in `mcp-servers-gateway.mjs`.
- [ ] **It fits at 800 and 390px** — nothing outside its card, nothing
  clipped (a field's value included: the command wraps), Settings not
  scrolling sideways, the fold held, and under a 420px page a server row's
  actions under its text (U29, U30). _Check:_ `responsive.mjs --only
  integrations-narrow` (whatever the page shows); with a row, the form and the
  inspection open over a real gateway, `mcp-servers-gateway.mjs --only narrow`.
  _Harmless:_ at 390px the window under Settings scrolls 90px sideways (its
  own 480px minimum), reported as `windowScroll`; Settings itself does not.
- [ ] **A server added here reaches a new conversation** (the issue's
  Done-when): added again from the window, its switch on, the agent asked for `show_chart`
  in a new conversation, its app is live and its frame shows the tool result's
  series, once. _Check:_
  `mcp-servers-gateway.mjs --only done-when` (needs the agent signed in on the
  machine). "Once" depends on #418's fix (#421) being in the tree.

## Icon buttons (#632)

One component, `ui/icon-button.tsx`, draws every icon-only control outside
Settings: a box of 26, 28 or 32px, rounded or a pill, named "Label (⌘K)" and
saying the same in its tooltip, filled by `--desktop-hover-strong` under the
pointer, and outlined by `--desktop-focus-outline` under the keyboard.

- [ ] **Every icon button holds the contract, and none of the replaced classes
  is left.** _Check:_ `icon-buttons.mjs` (`contract`, per engine and layout;
  `--layout classic` for the classic shell). Rule: `ui/icon-button.css`.
- [ ] **The pointer fills it with the stronger hover.** _Check:_
  `icon-buttons.mjs` (`hover`).
- [ ] **Keyboard focus on one draws an outline.** _Check:_ `icon-buttons.mjs`
  (`focus`). At the base commit the workspace's resets left these with none.

## Shared controls (#632)

Patterns the window drew several ways, each now one component of the kit
(`@nessa-ui/react`), given the window's inks in one rule. Settings is left out:
it is redesigned on its own branch.

- [ ] **Every key cap is the kit's `Kbd`, skinned once.** 18px tall and at least
  as wide, 11px medium type, `--desktop-radius-xs`, a 7% fill and
  `--desktop-muted` ink — the sidebar's search, the session list's search and
  the quick switcher's rows alike. _Check:_ `shared-controls.mjs` (`keys`, per
  engine and layout, on load and with the switcher open). Rule: `chrome.css` ›
  Keys. At the base commit the session list's and the switcher's keys were
  bare text, unlike the sidebar's cap.
- [ ] **The session list's search and the group captions are the kit's
  (#657).** Columns only — the sidebar layout folds the list, and the check
  holds without measuring it. The search is `SearchField`: 32px tall, 13px
  type, the window's 12px corner, and a missing input fails. A caption is
  `GroupHeader` (11px/600; 16.5px in the list, 27px in the switcher),
  `aria-hidden`, inside a `group` whose accessible name is that caption, so
  the listbox exposes groups and options. _Check:_ `shared-controls.mjs`
  (`list`, per engine). Rules: `session-list.css` › search and group label,
  `quick-switcher.css` › `.workspace-results-group`.
- [ ] **Every list row is one of two components, answering the same way
  (#657).** The sidebar's channels, its Agents entry and "Show all" are the
  kit's `SidebarMenuItem` (`xs`); the sidebar's sessions, the session list,
  the overview's sessions and the switcher's results are the window's
  `ListRow` (`ui/list-row.tsx`), where a row is a listbox's option or its
  trailing detail is as wide as it says. Under the pointer every kind fills
  with `--desktop-hover`, a chosen row with `--desktop-selected`, and an
  unread title weighs `--desktop-unread-weight`; a kit row's name says the
  count or glyph the kit lays beside it, which stays silent, and pointed at
  that glyph the row keeps its fill; a switcher result, whose choice
  follows the pointer, draws no hover of its own. _Check:_ `shared-controls.mjs`
  (`rows`, per engine and layout: on load, in the overview, with the switcher
  open). Rules: `ui/list-row.css`, `source-list.css` › Rows. At the base
  commit no row was either component, and unread weighed 560, 600 or 650 by
  list.
- [ ] **The window's name is one control in every place (#657).** "nessa
  Studio" at the workspace's and the classic shell's foot and "‹ nessa Agent"
  at Settings' are `ui/identity.tsx`: a pill the corner controls' size and
  radius, "nessa" and its word in `--desktop-muted`, named, filled with
  `--desktop-hover` under the pointer; Settings' row stands where the
  window's does. _Check:_ `shared-controls.mjs` (`identity`, per engine and
  layout; "‹ nessa Agent" measured inside Settings, over the inert window
  that keeps its own; and in the classic shell). Rule:
  `ui/identity.css`. Sampling waits for its finite hover transition through
  `waitUntilSettled`, retaining the exact fill assertion. Browser measurement
  ordering is in [UI workspace load](../../docs/design/ui-workspace-load.md#verification-sampling-order).
  At the base commit the classic shell's read
  "nessaStudio" with no name, no pill and no fill, in the kit's grey.
- [ ] **Every empty state is the kit's `EmptyState`, in the window's type
  (#657).** The session list's, the workspace's failure, an overview group
  shown alone, "Nothing needs you", the classic shell's notes, the subagents
  panel's and a widget's line: a quiet one (`compact`) a faint footnote line
  with its action 12px under it, a titled one at reading size, 600, its
  sentence muted. Where each sits is its surface's. _Check:_
  `shared-controls.mjs` (`empty`: the quiet ones — the session list with no
  match, the classic shell's notes); `gateway-states.mjs` reads the
  workspace's sentence from the kit's title. The titled ones ("Nothing needs
  you", the subagents panel's) show only with nothing listed, which the
  sample never is: their type is held by #657's before/after shots
  (`evidence/657/empty-*`, `rims-all-clear-*`), not by a script. Rule: `styles.css` › Empty states. Settings' own are left to
  its redesign.
- [ ] **The empty list's New Session and a failed workspace's Try Again are
  the kit's tinted pill (#657).** 28px tall, the kit's full round
  (`shape="pill"`, not the window's 999px pill), filled with
  `--desktop-selected`, and a press scales to 0.97. The old
  `.workspace-button`, a missing button, another corner, another fill, or a
  press that does not scale fails. _Check:_ `shared-controls.mjs` (`empty`,
  columns: the writing channel's one session is archived, then New Session);
  `gateway-states.mjs` (signed out, Try Again). Rule: `ui/pill-ink.css`.
- [ ] **Counts are the kit's `Badge`; lit points are `StatusGlyph`'s (#657).**
  A sidebar row's count is the kit's `Badge` as the row's caption (16px tall,
  at least 18 wide, no border, the needs light for what waits); the session
  list's unread point and the overview's "Needs you" point are
  `StatusGlyph`'s 6px points (`unread`, `needs-you`; `flush` where words start
  at the point, `decorative` beside words that say it). _Check:_
  `shared-controls.mjs` (`badges`, on load and in the overview; an unread
  point and the heading's point must be found);
  `status-glyph.test.tsx` for what each says. Rules: `source-list.css` ›
  `.workspace-badge`, `chrome.css` › `.workspace-status`. At the base commit
  the unread point and the heading's point were drawn by their own rules.
- [ ] **A card set into a surface has one edge (#657).** A code block, an
  agent's command, a widget's card and "Nothing needs you" take
  `--desktop-card-rim`, a 0.5px hairline at 10% of the ink; a pane's glass
  keeps `--desktop-rim`. _Check:_ `shared-controls.mjs` (`rims`: on load, in
  the widget sample, in the overview's peek; a code block, a command and a
  widget's card must be found; "Nothing needs you" shows only with nothing
  listed, which the sample never is: `evidence/657/rims-all-clear-*`). Rule: `styles.css` ›
  `--desktop-card-rim`. At the base commit the four had four edges (1px at
  4%, 1px `--desktop-rim`, 0.5px at 9% and at 10%). Settings' cards are left
  to its redesign; warm edges (a review, the peek's request) are their own.
- [ ] **A session's pane, a widget's pane and the window share one header
  bar (#657).** `panes/pane-header-frame.tsx`: name, spacer, what is added,
  actions, with `headerBar`'s drag attributes; a pane's × keeps its room
  (`ClosePaneButton`). _Check:_ `pane-header-frame.test.tsx`; the markup of
  every header is unchanged from the base (scratch comparison in #657's pull
  request); `drag.mjs`, `widgets.mjs` and `safe-area.mjs` hold the behaviour.
- [ ] **The overview's counts are the kit's `SegmentedControl` (#657).**
  Bare, in the line's own type: none pressed while every group shows,
  `--desktop-hover` under the pointer, `--desktop-selected` pressed, nothing
  moving as one is chosen, and the pressed one chosen again letting go. The
  side rail's lens and Settings' own controls stay with their owners.
  _Check:_ `shared-controls.mjs` (`segmented`); `overview.test.tsx` for
  what each count shows. Rule: `overview.css` › counts.

## Menus and tooltips

_ADR 238 › Interaction and visual rules_ holds the rules; check each by hand,
in both themes, and screenshot what a rule is about (a highlighted item, a
menu near the window's right edge, a pane header on hover).

- [ ] **Every rule in _Interaction and visual rules_ holds.** _Check:_ manual.

## Motion and reduced motion

_ADR 238 › Decision_, last paragraph ("Motion animates transform and opacity only").

- [ ] **Only transform and opacity animate**; workspace bounds land immediately,
  while independent surface and first-message motion keep their own durations. _Check:_ DevTools › Performance: no
  layout-inducing properties in the animation; `perf-budget.mjs` attribution
  shows forced layout per script.
- [ ] **Calm, no jitter**: nothing moves one way then the other within a
  transition. _Check:_ `drag.mjs` (`sweep-across-zones`, one-way); by eye
  with `--headed` and a slowed animation for the rest.
- [ ] **Reduced motion sets durations to zero** (Settings › Motion, or the
  system's), and script motion follows. _Check:_ `safe-area.mjs --reduced-motion`
  and by hand; with less motion, a drag moves only the copy: `drag.mjs`
  (`reduced-motion`), and `drag.mjs --reduced-motion` for every drag check
  with the system's reduced motion on.

## Themes, zoom and sizes

- [ ] **Every theme, light and dark, reads** — graphite, ocean, ember, dusk.
  _ARCHITECTURE.md_ (themes). _Check:_ manual screenshots of the same state per theme.
- [ ] **At a small window (960 × 600) nothing is clipped or under the
  controls**, overview and Settings included. _Check:_
  `safe-area.mjs --sizes 960x600`.
- [ ] **Browser zoom / larger text** does not break the titlebar row.
  _Check:_ manual (⌘+ twice in the app).

## WebKit parity

- [ ] **Every scripted check passes in WebKit too.** _Check:_ scripts default
  to `--engine chromium,webkit` where it matters; `perf-budget.mjs` is
  Chromium-only (CDP throttling, Long Animation Frames).
- [ ] **A WebKit-only difference is reproduced in the app itself** (Tauri,
  WKWebView) before it is reported as a bug; Playwright's WebKit is close,
  not identical (pointer events outside the viewport differ, for one).
- _Known:_ headless WebKit does not draw `backdrop-filter`: the window's glass
  (menus, tooltips, the revealed sidebar, the model picker) shows as its flat
  fill in headless WebKit screenshots. Judge glass with `--headed` or in the
  app, not from a headless WebKit shot.

## Load fallback

The panel's webview is a stage larger than its window, pinned to the window's
bottom right (`src/panel/adapters/panel-frame.ts`); before the frontend
mounts, `index.html` shows the fallback on that stage.

- [ ] **"Loading" sits inside the visible window and is centred in it once the host reports its size**, on the default frame, a short configured
  height, and a narrow panel; with the size pending or refused it stays inside the bottom-right 320 × 320 and still says "Loading". The screen is a dark field and that word. It does not paint the avatar, a glow, or any image, and nothing on it animates. When the frontend script is not served (the panel with no host, and setup), the calm screen replaces Loading: "Nessa couldn’t start", the code `STARTUP_MODULE`, and Restart and Quit as circular icon actions, still with no avatar. The page URL and the script path stay in the console, not on the screen. Setup centres that stack; the panel starts it at the window's top left. Nothing
  paints over the line, and the page does not scroll. _Check:_ `load-fallback.mjs` (runs the real frontend against
  a fake host whose startup never answers and which fakes `panel_size`).

## The window's gateway

The desktop app's window reads the local gateway over the panel's credential,
which its host serves it once the gateway is ready (#419). When it cannot, it
says why where the conversations would be.

- [ ] **Signed out says why in the chat area, with Try Again. A startup
  failure covers the window with the calm screen. Neither shows the sample.**
  A gateway refusing the credential says "This window isn’t signed in to the
  local server." The host refusing the credential, the gateway not ready yet,
  and no gateway listening each show "Nessa couldn’t start", the code
  `STARTUP_GATEWAY` from `src/host/startup-refusals.json`, and Restart and
  Quit as icon actions. The log sentences (started without the local server,
  still starting, not answering) stay off the screen. The avatar mark is absent.
  While the gateway is not ready the host refuses the endpoint and the
  credential is never asked for. Signed out, the status sits inside the chat
  area and the window, Try Again is at least 24px tall with nothing over it,
  and no session row or sample plugin is drawn. Try Again reads the index
  again (the status goes while it reads, which no poll does) and connects at
  once though the poller waits, and says the same while nothing changed. Try
  Again is told from the poller by a clock the script holds, not by timing:
  Playwright's clock runs the page's timers, in real time until the host has
  gone unasked for two poll rounds (the unasked spell), when it pauses. With
  no timer firing, a host ask after the click can only be Try Again's own
  connect; with none, Try Again did not connect, or joined a connect still
  in flight, or its connect waits on a page timer, which the paused clock
  holds. That rests on a premise: Try Again's connect reaches its first host
  ask with no page timer, as React's scheduler and IPC promises use none. If
  that stops holding, a correct product fails, never a broken one passes.
  The pause must also come while the poller still waits: under a round short
  of its wait after the last ask (the quiet), at least one refused round is
  still owed. Given that check, a broken Try Again fails, and a connect
  still in flight or a held timer can only make a correct one fail, never a
  broken one pass. It fails when the host is never unasked for two rounds
  within the poller's wait and three rounds more, when the host was asked
  within two rounds of the pause, when the pause came after the poller's wait
  could have ended, when no failed connect came before the click, when Try
  Again could not be clicked, when no ask follows the click, or when Try
  Again did not read the index again. Timing numbers that are missing or not
  positive could not run, and so could a quiet too short for the unasked
  spell and the lead before the pause to fit under it (C0–C4, #419 comment
  5977020094; C5, #419 comment 5978179804). A startup screen's Restart does
  not ask the host for the index; the poller does. Restart and Quit are at
  least 24px tall with nothing over them, and the screen covers the window.
  While signed out, and while the gateway is not ready, the window does not
  ask the host at all for a round short of the poller's wait after its last
  ask, then asks exactly once by three rounds after it: after a failed connect
  it waits out several poll rounds rather than asking the host every second,
  and then tries again on its own. The
  wait, `pollMs × reconnectRounds`, is read from the gateway source's own
  `defaultGatewayTiming` in the page (so the script needs `--mode dev`); the
  unit tests pin the rule, `reconnectRounds + 1` rounds, S10.
  _[Degrade honestly](../../CODING_STANDARDS.md#gates)._ _Check:_
  `gateway-states.mjs` (runs the real frontend as the desktop app, against a
  fake host whose endpoint and credential commands answer per scenario, and a
  fake gateway socket that refuses the credential as `product/socket.rs` does).
- [ ] **A conversation the window listed but could not read says what could not
  be read, in the open transcript and in the Agents peek.** The gateway
  answers the handshake and lists one running conversation, then refuses
  `conversation.read` as `temporarily_unavailable`. The transcript's note says
  "Nessa couldn’t read this conversation just now.", inside the chat area,
  and never the unconfirmed-call sentence. Agents then shows that conversation
  under Working; at a width that holds the peek beside the list, that peek
  says the same sentence, and at a width that does not, the peek opened
  beneath the row does. No console error, page error, or failed request.
  _[Browser verification for UI](../../CODING_STANDARDS.md#browser-verification-for-ui)._
  _Check:_ `conversation-unread.mjs` (the real frontend as the desktop app,
  against the same fake host as `gateway-states.mjs` and a socket that lists
  the conversation and refuses the read).
  _Not in a browser:_ a request row's unreadable. Needs you is a conversation
  whose read already asked the person something, so a first read that fails
  never lands there. `overview.test.tsx` holds that sentence.
- [ ] **A gateway that answers shows its conversations in the main window, and
  a turn made elsewhere without a reload.** Over the host's endpoint and the
  panel's credential (the file the gateway provisioned, which the native host
  reads), the window's socket goes to the host's endpoint, its handshake names
  client `nessa-panel` and is answered with principal `surface:nessa-panel`;
  the window lists the conversation by the title the gateway gives it, with no
  failure status and no sample; opened, its transcript draws the person's
  message and then the agent's reply, each exactly the text the gateway's
  `conversation.read` holds, inside the chat area; and a turn sent from
  another surface under the same credential, once the gateway holds it, is
  drawn in the open transcript as its last two messages, the page not
  reloaded (the gateway source's poller). Every handshake the window makes,
  each reconnect's too, is the first's: the host's endpoint, client
  `nessa-panel`, surface kind `desktop` with an instance, principal
  `surface:nessa-panel`. No console error, page error
  or failed request at any point. In Chromium and WebKit. The reply compared
  is a text-only one (only text parts, plain text, not empty), which the
  window draws as its text alone; an agent that answers otherwise leaves the
  steps "could not run", not failed.
  _[Browser verification for UI](../../CODING_STANDARDS.md#browser-verification-for-ui)._
  _Check:_ `gateway-window.mjs` (runs the real frontend as the desktop app —
  `hostGateway`, `connectDevSession`, the gateway source — against a fake host
  whose endpoint and credential commands answer with a real gateway's, started
  by `lib/gateway-stack.mjs` with a real agent; needs the agent signed in).
  With `--scripted` the same steps run signed out against
  `scenarios/text-reply.json`, including `--mode prod`.
  _Not in a browser:_ the native host's side — readiness, endpoint discovery,
  the credential file read and its refusals — is the Rust host tests'
  (`src-tauri/src/surface_credential.rs`,
  `src-tauri/src/gateway_endpoint/entrypoint/command.rs`,
  `src-tauri/src/gateway/infrastructure/commands.rs`); WKWebView's own IPC and
  the packaged app's `tauri://localhost` origin are the live `pnpm app` run's.
- [ ] **A gateway that answers shows its servers' MCP Apps in the main
  window.** The window, over the host's endpoint, draws the test server's
  `review_rows` app inline once that turn is in the conversation: one frame,
  live, its document the server's. _#574._ _Check:_ `gateway-window.mjs`'s
  `apps` step (dev server; `--mode prod` leaves it out, MCP Apps need the
  dev server's sandbox). With `--scripted` a prompt containing
  "show the server's app" is the text-reply scenario's `review_rows` turn,
  in that page's own conversation. The packaged app's own scheme
  stays the live `pnpm app` run's.
- [ ] **A scenario can ask, fail mid-turn, and wait to be cancelled, and the
  window shows it.** _Check:_ `scripted-scenarios.mjs` (and
  `pnpm test:e2e:scripted`, which also runs `mcp-apps-gateway --scripted` and
  `gateway-window --scripted` in Chromium and WebKit). The agent runs
  `scenarios/window.json`: streamed text, an agent approval answered with the
  review's own allow option (Always Allow is not offered), a tool step, a turn
  that fails after speaking, and a turn that stays cancelled after the
  conversation is closed. The strings come from that file.
  `--mode prod` runs this check and `gateway-window --scripted` against a
  production preview; MCP Apps stay on the dev server. One evidence directory
  per run, and one verdict line.
  _[Browser verification for UI](../../CODING_STANDARDS.md#browser-verification-for-ui)._

## Console errors

- [ ] **Observed resource failures retain exact read and mount identity through
  final reporting (#996).** The dev `gateway-window` apps check requires all
  three factory decorators and a real accepted live read. Only the same Request's
  HTTP200 `net::ERR_ABORTED`, complete consumed EOF, matching length/SHA256,
  original client acceptance and its owning bridge uncanceled and live at the
  original browser observer cutoff qualifies. Request completion is checked
  through the later Node final collection cutoff. Pending, invalid, canceled
  before browser cutoff and duplicate proof bypasses all older size/live patterns.
  _Check:_ `resource-read-regression.mjs` exercises real factories in Chromium
  and WebKit with synthetic HTTP/ports; simulated aborts are explicitly labelled.
  Later successful `resources/read` calls on an already-live bridge qualify
  without another view callback, including concurrent reads. Corrupt, truncated,
  stopped and replaced original-owner reads remain invalid; a live replacement
  cannot lend proof to them. `resource-read-liveness.test.mjs` additionally gates
  both acceptance/view orders and cancellation before and after acceptance.
- [ ] **Opening failure evidence precedes page disposal (#981).** A failed
  gateway app open keeps bounded animation, post-failure RAF, redacted unfinished
  HTTP/RPC state and a screenshot. The original readiness predicate, 3000ms
  animation deadline, failure and subsequent not-run checks stay authoritative.
  _Check:_ `resource-read-regression.mjs`'s finite-animation failure control;
  diagnostics do not attribute the historical failure's cause.

- [ ] **No console error, page error or failed request** while any script
  runs. _Check:_ every script adds them to its failures; `smoke.mjs` reports them
  per layout as `console`.
- [ ] **Stress opens the exact created conversation.** Repeated titles across
  browsers must not redirect its window actions. Use the existing exact-ID row
  selector with the title constraint; a missing ID fails rather than opening
  another same-title row. Preserve the15-second opening/composer deadlines.
  _Check:_ `lib/stress-conversation.test.mjs`, default and raised stress runs.
- [ ] **Home stress settles the conversation created in its focused window.**
  Read its existing selected list row's identity after the UI send; a prior
  same-title row cannot substitute while publication is pending. Require one
  selected matching row, retain its ID for settlement and the final rest sweep,
  and preserve the30-second discovery deadline and three-client/24-create load.
  _Check:_ `lib/stress-conversation.test.mjs` and both-engine default and raised
  `conversation-stress.mjs` runs (#1026).
- [ ] **Stress restart errors belong to the planned gateway outage.** Install
  socket observation before navigation. Only exact owned `/session` refusal/reset
  console occurrences during the action, corroborated by actual socket errors,
  a new receiving connection after the latest observed error that remains live
  through successful existing recovery assertions, may be
  retained as harmless. Older duplicates, later errors, another endpoint or cause,
  missing evidence, failed recovery and overflow stay failures. No permanent
  pattern is installed. Recovery requires the current successor userText and its
  completion together in one existing60-second window poll. _Check:_ `lib/stress-restart.test.mjs` and the default
  and raised `conversation-stress.mjs` runs in Chromium and WebKit (#1024).
  The current WebKit SIGKILL probe records the exact reset dialect
  "The network connection was lost." on both the owned socket and console
  inside this scope, followed by a distinct receiving connection. That exact
  cause uses the same bounded observation owner; it is never a permanent
  console exemption. Before correction, default behavior/followers passed but
  this console check failed (59/60); the focused probe reproduced it (4/5).
  A subsequent current linking-on probe separately pairs "Could not connect to
  the server." on the same owned socket and console before a new receiving
  connection. Its exact refusal dialect uses the same owner; default59/60 and
  observation-only WebKit29/30 failures are retained before that correction.
- _Harmless:_ the first page in a fresh browser asks for `/favicon.ico` and
  gets a 404 (matched by source URL in `lib/selectors.mjs`,
  `harmlessConsole`). Chromium reports a request as failed with
  `net::ERR_ABORTED` in two cases, and `ERR_ABORTED` is not ignored as a
  class (`lib/browser.mjs`, `recordFailedRequest`):
  - a 200 on the page's own origin whose `content-length` was fully
    delivered (`sizes().responseBodySize`). The client's read of
    `/mcp-resources` is the one that shows this (#473). The gateway waits
    for a size report already in flight before it reads the line. Until
    that size is known the line stays an error. Its release step may move
    an unlabelled same-origin `/mcp-resources` abort to harmless only when
    that pane's mount went live and the inline mount stayed live. A
    cross-origin abort stays a failure.
  - a 204 on the page's own origin, whose body is empty and never read:
    the window's sign-in check `/browser/check` (#485).
  With no response, a short body, any other status, another origin, another
  error, or a page with no origin, it stays a failure (rows F1′ and F2–F4).
  Whether the bytes were right is each check's own assertions' to judge
  (`renders` in `mcp-apps-gateway.mjs` for `/mcp-resources`; the conversation
  loading at all for `/browser/check`).
  Harmless lines of either kind are kept in the JSON as `harmless` by
  `lib/page-lines.mjs`, which every `openPage` watches once `run.mjs` has
  bound the check's reporter (`verification/desktop/page-lines.md`). A
  step's result takes the lines so far; close reports what is left as
  `console`. A script does not splice the arrays. Vite's
  `[vite] connecting…` / HMR messages are logs, not errors. A reload caused
  by another edit landing on the dev server mid-run is not a finding —
  re-run.

## Where a conversation runs

- [ ] **The details sheet says where the agent runs and the lease's status, for
  every lease state the gateway publishes, read in full.**
  _Check:_ `lease-details.mjs` mounts the production details sheet over each
  published lease (none, live, ending, ended, interrupted, refused, unreadable,
  and live, lost and refused on an SSH host and on a paired machine), decoded
  by the actual client validator, at 360, 600 and 1440 px. It asserts the
  "Where it runs" rows in order (a refused lease shows only its status, and
  the host it was asked of; an unreadable one claims nothing known; an SSH
  host is named in full; a paired machine is "Paired machine" and the first
  eight hex digits of its key),
  that no value is cut short, and that the section stays inside the viewport.
  Fixture map: [conversation verification](../conversation/README.md).
- [ ] **A new conversation can be put on an SSH host the gateway names, or a
  paired machine connected when it asks, and only then.** _Check:_
  `lease-details.mjs` (`run-on-none`, `run-on-hosts`, `run-on-paired`)
  mounts the production composer tray: with no host there is no "Run on" row;
  with hosts it lists this computer (checked and focused) and each host in
  full, marked SSH, then each paired machine as "Paired machine" and the start
  of its key, marked Paired; a choice is reported by its full name and shown
  on the row, and nothing scrolls sideways. The tray is the only place a host
  is chosen; the gateway refuses a host its `config.json` does not name, and a
  paired machine that is not connected. The fixture owns its viewport height;
  the same checks measure its height and bottom against the viewport (within
  1 px), keeping the tray at its foot without scanning test utilities into the
  production CSS (#929).

## Restored local tab lifetimes

- [ ] **Restoration creates a fresh local tab while preserving its durable target.**
  Finishing the original close affects the original remote conversation; the
  replacement owns its follow and has no old cancellation notice. Same-remote
  and different-remote replacements are measured at 360, 600 and 1440 pixels
  in both engines.
  _Check:_ `local-tab-lifetimes.mjs`; ordering rows LT1/LT6/LT7 in
  [local tab lifetimes](../../docs/design/local-tab-identities.md). Follow,
  stop and close counts are read from the fixture owner
  (`__localTabLifetimes.snapshot` in
  [live owner reads](../../docs/design/live-owner-reads.md)), not from
  attributes the last render copied.

## Committed transcript controls

- [ ] **Unconfirmed history has one notice and no offered controls.**
  _Check:_ `committed-transcript.mjs` mounts the production conversation controls
  and `applyView`; covers partial, stale, unknown and not loaded, complete empty,
  complete without live attachment, and complete with offered authority, including
  re-enable after incomplete history. Server projection tests own execution identity.
  Fixture map: [conversation verification](../conversation/README.md).

- [ ] **Empty display does not authorize saved-tab omission or gateway close.**
  _Check:_ `committed-transcript.mjs` feeds published view states through the actual
  decoder, Redux read projection, saved-tab selection and close thunk. Partial,
  not loaded, stale, unknown and complete-zero preserve one saved reference and
  dispatch zero gateway closes; confirmed complete-empty permits omission and one
  close. A truncated complete-empty view still preserves the reference. Both
  engines and all three widths assert the original local tab closes in every case.

- [ ] **A question answer belongs to the conversation whose fields were shown.**
  _Check:_ `committed-transcript.mjs --mode prod` renders the production
  `ConversationQuestions`, store controls and client serializer in Chromium and
  WebKit at 360/600/1440 pixels. Two conversations share turn/question IDs:
  three offered fields remain operable, an equal-key simultaneous ask retains
  its middle selection, switching transfers zero selections,
  and exactly two serialized actions retain their original targets (valid
  offered choice, then null decline). Source projection/recorder evidence;
  gateway/SDK tests own current-state admission. The primitive and ordering
  table are [question targets](../../docs/design/question-targets.md).

- [ ] **A Markdown table in a reply keeps every word whole and scrolls when wide.**
  _Check:_ `conversation-stress.mjs`'s `table` step (knob `tables`) sends from
  the window's composer; the scripted agent streams a narrow and a wide table.
  Once the last cell is drawn and the reveal has finished, both tables show
  their data rows once each and no short run ("Price", "$16.99", "Runtime",
  "Format") is split across lines, even under the transcript's
  `overflow-wrap: anywhere`. The narrow table fits the reply; the wide one
  scrolls in its own panel, fades its right side (`data-hidden-right`) and
  moves the fade left once scrolled to its end. The gateway's view holds the
  answer exactly as streamed. Chromium in the PR job; WebKit with the stress
  load run. The table itself is nessa_ui's `MessageMarkdown` / `Table`
  (`edgeFade`), with its own story checks.

### Onboarding readiness recovery

`node verification/desktop/scripts/onboarding-readiness.mjs` drives the real
setup UI, controller and HTTP adapter in Chromium and WebKit against controlled
HTTP responses that never send headers or never complete the JSON body. It
measures release at the ten-second request deadline (9.9–12 seconds including
browser scheduling), verifies the aborted first request, an enabled and painted
Check again control, and exactly one healthy retry that makes Claude ready.
Both cases require zero page errors. The rule is owned by
`src/onboarding/adapters/agents.ts`; ordering and unit regressions are in the
[adapter tests](../../src/onboarding/adapters/agents.test.ts) and
[setup recovery tests](../../src/onboarding/ui/onboarding-readiness-timeout.test.tsx).

### Linux linger at setup

`node verification/desktop/scripts/linux-linger.mjs` drives the real setup UI
and controller in Chromium and WebKit. The fixture supplies the host's linger
answer; it does not call logind. After the shortcut, an offer shows the choice
and does not say the gateway keeps running after logout, and it does not ask
for an administrator. Accepting shows that sentence only when the answer is
`enabled`. Declining finishes setup without a call. A refusal and a failure
name `loginctl enable-linger` and claim nothing. An unsupported host claims
nothing. An account that is already lingering is reported without a call. Both
engines require zero page errors and painted controls. The screen is
[ADR 217](../../docs/adr/done/217-linux-linger-at-setup.md). The live bus is
`scripts/desktop/check-linux-linger.sh`, for user `lt` only. After the PAM
login session ends, the user is `lingering`, `user@<uid>` is active, and
`sleep infinity` is still running. A per-user `systemctl` machine match is
not that proof: it starts the user manager itself. `terminate-user` stops
that manager even when the linger file is present, so it is the negative
control: with linger then disabled, `user@` is inactive and the process is
gone.

### Attachment admission races

`node verification/desktop/scripts/attachments-races.mjs` drives the actual App,
Redux store and Markdown editor in Chromium and WebKit, with controlled native
picker/read, host drop delivery, image HTTP responses and scenario gateway/session
effects. The fixture supplies a digest and session readiness; it exercises no real
gateway, provider or authentication. A healthy Enter
control sends exactly once. Enter during an unresolved native image read sends
zero times, preserves the draft and paints **Attachments still loading**; read
completion allows one send containing the text and one image. A second URL drop
while the first fetch is pending paints **Still reading files** and starts no
second fetch. The JSON reports send, attachment, refusal and page-error counts
for all forty engine/scenario pairs. The `drop-capacity` case begins 64
batches in A/c0, switches to empty B/c0, and checks that the visible 65th-drop
refusal belongs to B. The rejected overflow and unknown results do nothing;
the oldest result attaches once to A, a duplicate does nothing, and B accepts a
new drop after the terminal result frees capacity. Committed feature evidence
for this flow records the source head, Chromium and WebKit screenshots, the
concise matrix output and the source-revert result in
[`attachment-drop-capacity`](evidence/attachment-drop-capacity/README.md).
Overlapping picker requests are
announced and resolved in reverse order across two providers with the same
conversation ID; each readiness tile and returned file stays with the provider
that started it. A late upload success or failure for a closed
parent leaves the replacement parent's shared-resource tile uploading (one rendered
`data-upload="uploading"` tile), per [attachment lifetimes](../../docs/design/attachment-lifetimes.md).
The same-parent remove/re-add cases also retain one uploading tile after an old
success/failure. Shared bytes receive two distinct placement uploads; removing
one aborts exactly one, keeps the other alive, and releases bytes only after the
last parent closes. A Provider replacement gives the new store's `a1` its own
upload: the old completion leaves its single rendered tile uploading, and only
its own reply makes that tile stored. An open original preview disappears when
the owner changes, despite matching local conversation/placement numbers. Unmounting
with three active uploads leaves the fourth queued after a completion; remounting
resumes that sibling. Native SVG bytes retain their MIME type and render a 4px
image from the object URL in both engines. This verifies browser interactions with the
production panel; it does not exercise an operating-system file picker. The
`folder-owner-send` and `folder-owner-drop` keep a same-ID pending folder walk
in A from blocking B's immediate send or next folder gesture; each settled walk
still writes to its captured provider. The JSON reports send, attachment,
refusal and page-error counts for all forty engine/scenario pairs.
`pathless-host-drop` case proves prose and image URLs delivered without a batch
beginning still reach the composer and the destination captured before the image
fetch waits. `browser-picker-fallback` cancels and reopens the native browser input,
then resolves its delayed file change after the provider has changed; the original
owner receives it. Cropped views of both results are retained in
[`attachment-gesture-owners`](evidence/attachment-gesture-owners/README.md).
`conversation-refusals` leaves one refused folder in each of two open tabs,
delivers A's result while B is active, revisits and answers A, and verifies B's
notice remains. The paired screenshots show each tab's own notice. A delayed
host-batch registration test also replaces the provider before the subscription
resolves and verifies the single mounted listener still captures the current
destination; a retained callback after unmount has no effect.
AL24 also covers a terminal refusal followed by a restored-draft retry. In each
engine, the fixture stages a 40 MiB native PNG as a 68 B processed image, refuses
the first send, retries it successfully, then accepts a second 40 MiB image. Both
reupload modes are exercised: the refused bytes remain stored, or the refused
image is staged again. The 80 MiB of originals crosses the 64 MiB sent-preview
cap; checks tied to the send execution IDs confirm the old failed turn becomes a
reference, the older Blob and object URL are released, and the second file remains
available. When the refused reference is no longer held, the transcript shows the
existing **No longer kept** placeholder. The bounded run is
`node verification/desktop/scripts/attachments-races.mjs --only terminal-refusal-preserved,terminal-refusal-reupload`.


## Usage limits (#1075)

`node verification/desktop/scripts/usage-limit.mjs --engine chromium,webkit`
loads `fixtures/usage-limit/`, a dev-server page that mounts the desktop
transcript and the panel transcript on one view. A timed reset that the
schedule accepted says **Auto-resuming** and the same local time on both
surfaces. An unknown reset says **Reset time unknown**. A user action says an
account change is required and does not invent a time. A timed reset the
schedule did not accept, with the preference on, says **Scheduling failed.**
The checkbox label is **Auto-continue this project's threads when limits reset**.
It shows the gateway preference, stays enabled, turns off when the person
unchecks it, and snaps back when that change fails. A pure limit does not say
the turn ended early and does not show the required-work sentence. At 360px
and 1000px the card stays inside its surface.

## Provider authentication recovery (#501)

`node verification/desktop/scripts/provider-sign-in.mjs --engine chromium,webkit`
also verifies optimistic sending/failed delivery, accepted pending inputs,
user-only running and output without reviving recovery or duplicating the retry. It
checks both workspace and floating panel against [ADR 501](../../docs/adr/todo/501-provider-authentication-recovery.md): at
360px and 1000px the card fits its pane, height is at most 130px, its button stays
within the card, keyboard activation opens the selected provider once, an
acknowledged launch keeps the card, a failed launch says so, and a newer
transcript removes the card. Both browser engines report geometry, launch counts,
and zero page errors. The raw authentication diagnostic is absent and unrelated
local notices remain. On the panel, an authentication diagnostic folded into
the turn's status stays off the card unless `requiredWorkFailed` is set, in
which case the required-work sentence is shown once beside it. On the desktop,
a local retry that hides the card shows the authentication terminal once, and
a failed retry still shows it.

The same script drives an older failed local submission through a later wire
refusal, a newer local submission after seeing that refusal, repeated replacement
views, and two retained locals superseded by another refusal. Both surfaces read
`src/provider-authentication/model/recovery.ts`: observed execution identity,
including accepted pending inputs, owns recovery rather than array-tail position
or clocks.


## Active message synchronization (#532)

- [ ] Ready active text reaches the desktop DOM plus two animation frames within
  600 ms in Chromium and WebKit, in both layouts. A held summary list and another
  conversation's held read do not delay delivery. One background read per
  conversation, at most four active reads per second, one-second summaries and
  no idle transcript reads. Running/queued text keeps fast polling after an idle
  send even while its summary still says idle (#616). `run-all.mjs --mode prod`
  gives this check its dedicated fixture build. `message-sync.mjs` measures these contracts over the
  production source, store and window with controlled gateway replies. See the
  [ordering table](../../docs/reviews/startup-latency.md#desktop-transcript-delivery-experiment-532).
  Three fresh-page runs report delivery median/max and frame attribution. Chromium
  uses calibrated 4x CPU throttling and checks the 50 ms frame budget through
  `lib/perf.mjs`; WebKit delivery is reported separately without CPU throttling.
  DOM plus frame opportunities do not measure transport, provider startup or compositor paint.

### Retained widget and drag resources (#616)

- [ ] The same inline widget keeps its proxy/app document and local state when
  surrounding transcript parts are added or removed (`message-sync.mjs`, retained-app).
- [ ] Resize removes a suspended returning drag copy in the resize event's turn,
  including a blur followed by resize (`drag.mjs`, return-interrupted).
  [ADR 238](../../docs/adr/done/238-desktop-workspace-frontend.md) owns the flight ordering.

## Confirmed approval-mode publication (#712)

`mode-publication.mjs` starts its own real gateway with the scripted ACP profile
and drives the production panel effects/store/ComposerTray and workspace Source
in Chromium and WebKit. The fixture is served in dev; run-all labels it dev-only
in production. It creates an idle conversation and sends no transcript output.

- A verified Ask-to-Auto commit advances public read revision, updates
  the visible tray through its actual control, and advances the Source transcript
  publication count.
- Repeating confirmed Auto and unchanged reads retain public/panel/Source
  revisions. Returning to Ask advances them again.
- Runtime, capabilities, lifecycle and content remain unchanged across the
  mode-only change. No client-side forced acceptance or fabricated revision is
  used. The check closes its own page, client and gateway stack.

Ownership and negative/recovered cases:
[conversation admission](../../docs/design/conversation-admission.md).


## Panel Messages catalogue follow (#722)

`panel-list-follow.mjs` runs a real private scripted gateway and mounts the
production ConversationList, effects, history store and client subscriptions
in Chromium and WebKit. It is dev-only in run-all. The external caller uses a
second authenticated connection under the same panel principal.

- External create/send, archive and unarchive update the already-mounted list.
  An archived held tab is excluded by explicit archived-list evidence. Catalogue
  deletion removes the remote row; omission alone does not mark a held tab deleted.
- Active and archived initial frames are required before first publication; they
  are independent bounded replacements, not an atomic catalogue snapshot.
- A controlled failure at the archived application port remains visible while a
  real active-list frame adds another conversation. New paired follows recover it.
  This injected outage does not claim to exercise a real gateway fault.
- Leaving closes both target follows; an external change while hidden applies
  nothing. Reopening catches up through two subscriptions. Normal external
  changes retain the original mounted pair; no one-shot list reads occur.
- Small screenshots show the live list and stale notice. The script closes its
  page/client/gateway and deletes its conversations.

The [panel-list ordering table](../../docs/design/record-subscriptions.md#panel-messages-list--issue-722)
owns initial-half, stale-read, command, cleanup, incomplete and retry cases; the
source-level regression tests cover those cases independently.

## Published files (#701)

`published-files.mjs` loads `fixtures/published-files/`, a dev-server page
that mounts the production transcript in the desktop window and in two
420 px panels over a fake gateway. Pictures are read back through the real
attachment client, at 30 ms for each range. The script runs in Chromium and
WebKit and is dev-only in run-all.

- [ ] **Every published file's card stays inside its transcript's column, and
  a picture is painted at its own size, verified.** _Check:_
  `published-files.mjs --only layout --shots <dir>`. In both surfaces, each
  card's left and right edges are within 0.5 px of its column. `noise.png` is
  decoded from a `blob:` URL at 160 × 120 and painted at 160 × 120 CSS px.
- [ ] **A window reads at most two held files at once, and every picture still
  arrives.** _Check:_ `--only queue`. Each surface's peak count of reads at
  the gateway is between 1 and 2, and all eleven pictures end up shown.
- [ ] **A file the conversation no longer keeps says so, with no Retry. A file
  it could not read now offers Retry, and the retry paints it.** _Check:_
  `--only states` (screenshots of both cards).
- [ ] **Save writes the published bytes exactly, under the published name, and
  never through a link in the page.**
  _Check:_ `--only save,save-host`.
  - In a browser, Save `notes.txt` and Save `noise.png` each produce a
    download that is byte-for-byte the published file, with no `a[download]`
    left in the page. Saving a file no longer kept says "Couldn’t save".
  - With the desktop host stood in for, Save begins one host save. Every part
    it sends is at most 1 MiB, the finished file is the published bytes, and
    nothing is given up.
  - The page never inserts an `a[download]` and no download starts.
  - The host's own bounds (1 MiB a part, 64 MiB a file, a save let go after
    60 s idle, the temporary file renamed into place only once whole) are
    asserted on real files by `cargo test -p nessa-app -- saving`.
- [ ] **A conversation with no turn, only a file published while none ran,
  shows that file and not the empty welcome.** _Check:_ `--only empty`.

## Turn outcomes

`turn-outcomes.mjs` loads `fixtures/turn-outcomes/`, a dev-server page that
mounts the desktop window over a fake gateway and the panel's transcript of
the same view. One turn failed before it replied, one was cancelled after a
partial reply, one was left unresolved with no diagnostic, and one is still
running and asking three questions. The view also carries the gateway's
notice that a review was left out. The script runs in Chromium and WebKit
and is dev-only in run-all.

- [ ] **A failed, cancelled, or unresolved turn stays visible, with the
  gateway's status and diagnostic apart from a message that was not sent.**
  _Check:_ `turn-outcomes.mjs --only terminals`. The desktop shows Failed
  with "Provider connection lost", Cancelled with "Stopped", and Unresolved
  with no diagnostic. "Check the deploy.", "Halfway", and "Wait." stay.
  Nothing says "Not sent" for those turns.
- [ ] **The gateway's withheld-review notice is shown in its own words, and
  the questions it listed are on both surfaces.** _Check:_ `--only asked`.
  The notice is "A review was left out of this view." Both surfaces show
  "Where should this land?", "When should it ship?", and "Who should hear?".
- [ ] **Answering or skipping a question, or a later frame that no longer
  lists one, drops it on both surfaces.** _Check:_ `--only cleared`. Choosing
  Staging and Answer drops the first question. Skip drops the second. A frame
  that no longer lists the third drops it on the desktop and the panel.

## Transcript notes

`transcript-notes.mjs` loads `fixtures/transcript-notes/`, a dev-server page
that mounts the production transcript in the desktop window over a fake
gateway and in a 420 px panel; the panel applies the same view as its follow
does. The view has the gateway's shape: two turns, and two inputs still
waiting, each listed both as a queued turn and as pending. It has seven
notes: one in each state, from the owner and from someone the conversation
is shared with, one left before any turn, one after a turn the bounded view
left out (older than every turn it keeps), one left between the two waiting
inputs, and one whose text is a single word of over 300 characters. The script runs
in Chromium and WebKit and is dev-only in run-all. The rule it holds is
`placeNotes` and `TranscriptNote` (`src/conversation`), which both surfaces
use.

- [ ] **A note is a row of its own, right after the turn it followed.**
  _Check:_ `transcript-notes.mjs --only placement`. On both surfaces, notes
  and what the turns say read, top to bottom: the note left before any turn
  and the note after the dropped turn, the first turn, its two notes in the
  order left, the second turn, its two notes, the first waiting input, the
  note left after it, and the second waiting input. No note is inside a chat
  bubble or a turn's reply.
- [ ] **Each note says who left it, from where, and where it stands, in the
  same words on both surfaces.** _Check:_ `--only labels`. "Note left by the
  owner, from desktop" or "Note left by someone it is shared with, from
  phone"; no state for a recorded note, then "Waiting for the next turn",
  "Given to a turn", "Not delivered: expired" and "Not delivered:
  sharing changed".
- [ ] **A long note wraps inside its row and nothing scrolls sideways.**
  _Check:_ `--only wrap --shots <dir>`. Every note's left and right edges are
  within 0.5 px of its column, and its `scrollWidth` is at most its
  `clientWidth` + 1. The long note's text runs over more than two lines. The
  columns and the page have no horizontal overflow. The screenshots show both
  transcripts.
- [ ] **A bounded view that left older history out says so above the messages.**
  The fixture's view is truncated and complete. The desktop transcript says
  "Some conversation content is not shown." once, before the first message,
  and does not say the history is unconfirmed. "Here is the plan." appears
  once. _Check:_ `transcript-notes.mjs --only history`.

## Sharing a session (nessa_ui#128, #129)

`share-dialog.mjs` opens the dev server's page with `?gateway` against a
scripted product session that lists one conversation already shared Drive
with one linked device, and answers `conversation.shares` a page at a time,
`.share`, `.unshare`, `credential.list` and `agents.list` (four command hosts). It runs
in Chromium and WebKit. The rules are the gateway's; the dialog's order is
the table in `docs/design/read-grants.md` › "Dialog state and order".

- [ ] **Share… on a session's menu opens one dialog for it, focus inside,
  each grant said in words.** _Check:_ `share-dialog.mjs` (`open`). The title
  names the session; the existing grant reads "Drive · Read-only tools,
  commands on devbox".
- [ ] **A Drive grant is sent as the wire takes it, and listed as the
  gateway reads it back.** _Check:_ `drive`. One `conversation.share` with
  `{ preset: "no_shell", commandHosts: ["devbox", "ci-runner"] }` and a
  request id and `expectedPolicyRevision` 0 (a first grant); the count reads
  "2 of 16"; the notice says "Shared."; the
  notes say what a policy does not stop (unasked calls, hosts named and
  allowed or nowhere, Ask mode / hooks off / not OpenCode).
- [ ] **The hosts keep their own bound however full the tool patterns are.**
  _Check:_ `budget`. Both pattern lists full (16 and 16): "4 of 16", Share
  able to send; nothing sent. A host past the byte bound stays unavailable
  in `share-dialog.test.tsx`.
- [ ] **A refusal is said in words, never a wire code.** _Check:_ `refusal`,
  and every step's scan for codes.
- [ ] **While a change is on its way, and the read after it, the form,
  Share and Revoke are held, and take input again once that read answers.**
  _Check:_ `held` (row D10), which holds the gateway's share answer and then
  its read; the footer says it waits on this conversation's own change
  (`this-session`), and says no reason while the read after it is on its
  way, since the list then predates the share (row D10).
- [ ] **After this window's own change, a failed read leaves the list held,
  not stale and actionable.** _Check:_ `stale` (row D10). The read after an
  Update is refused: the dialog reads as `held`, the form, Share, Change and
  Revoke are disabled, no reason is said and Start Again is not offered; Try
  Again reads the list, which shows Comment, with no reason.
- [ ] **Revoke asks first, then sends one unshare naming the revision it
  read, and the row goes when the list is read again.** _Check:_ `revoke`
  (`expectedPolicyRevision` 2).
- [ ] **A change built on a grant another window changed since is refused
  in words, nothing changes, and Start Again builds on what it holds now.**
  _Check:_ `changed` (rows D5, D10). The phone's grant is changed to Comment
  elsewhere (revision 3) after this window read revision 2; the share naming
  2 is refused `share_grant_changed`, the notice says it "changed since this
  list was read" (no cause named: it may be this window's own change), Share
  is held with the reason `changed`; Start Again shows Comment, and a share
  naming 3 answers "Shared.".
- [ ] **Tab stays in the dialog; Escape closes it and focus is back on the
  session's row.** _Check:_ `focus` (waits for that end state, 3 s).
- [ ] **A pinned session under a folded sidebar gets that focus on the list's
  row, not the inert sidebar copy.** _Check:_ `focus-pinned` (the sample
  workspace, which can pin; the gateway keeps no pin).
- [ ] **The panel fits the window.** _Check:_ `fit-1200`, `fit-420`; with
  `--shots <dir>`, screenshots of the grants, Drive form, budget, refusal,
  changed refusal and the narrow panel.
- [ ] **With no linked device and nothing shared, the dialog is one line.**
  _Check:_ `empty`. "No linked devices yet." and Link a Device, no Done and no
  Share; the link closes the dialog and opens Settings on Linked devices.

## Session rows: hover, menu, rename, delete, swipe

`session-rows.mjs` runs against the dev server's in-memory workspace, in the
session list (`session-list.tsx`, `session-row-actions.tsx`). The swipe is
nessa_ui's `SwipeRow`, with the mouse drag and the context menu left to the
row.

- [ ] **Under the pointer, Archive and More take the time's place.**
  _Check:_ `hover`. Away from the row its time shows and no button does;
  under it the two show at 26 px and the time does not.
- [ ] **More holds what the row can do, less the ways to open it; Rename
  edits the title where it stood.** _Check:_ `more`. Rename, Pin to Sidebar,
  (Share…,) Archive, Delete…; the row keeps its buttons while the menu is
  open; the field takes focus and ↩ keeps "Rain plan".
- [ ] **Delete asks first.** _Check:_ `delete`. Cancel deletes nothing, and
  the menu opens again after the question and stays open; Delete removes the
  row.
- [ ] **The Archive button archives without opening the row.** _Check:_
  `archive`.
- [ ] **Two fingers left uncover Archive and Delete, as Mail's rows do.**
  _Check:_ `swipe`. A mouse drag does not slide the row (it is the drag to a
  pane) and right-click opens the row's own menu; a swipe rests the row open,
  slid aside, with Archive and Delete, and opens nothing; Delete asks
  "Confirm" in place, then removes the row; a long swipe archives. On a Mac
  trackpad by hand: the swipe follows the fingers and a flick rests open,
  never archives. The mouse check waits for the published prepared ghost and
  then its carrying mark before release, and keeps the captured row identity:
  an unprepared flick is a click, which can retarget a live unselected-row
  locator. The original 10/11 run and lightweight wrong-row trace failed;
  anchoring alone exposed two no-open failures (4/5). The private prepared
  check and exact restoration held 5/5; omitting readiness failed 4/5, while
  omitting the anchor with readiness held 5/5 (not a negative control).

## A record the chat could not read

`transcript-unreadable.mjs` loads `fixtures/transcript-unreadable/`, a
dev-server page that mounts the production transcript in the desktop window
over a fake gateway and in a 420 px panel. The view has one completed turn
and two records the read could not apply: one before any turn, and one after
that turn. The script runs in Chromium and WebKit and is dev-only in
run-all. The line is `UnreadablePartLine` (`src/conversation`), which both
surfaces use.

- [ ] **A record that could not be read is one line, and the turn around it
  stays.** _Check:_ `transcript-unreadable.mjs --only line --shots <dir>`.
  On both surfaces the reading order is the line for record 3, "Plan the
  release.", "Here is the plan.", and the line for record 9. Each line says
  "Couldn't read this part of the conversation", has an empty
  `data-unreadable-report`, and contains no button. The empty welcome, the
  whole-chat unread sentence, and the omitted-history sentence are absent.
  The screenshots show both transcripts.

## User attachments in desktop reads — #682

`user-attachments.mjs` drives the production gateway projection and desktop message renderer at 360, 600 and 1440 pixels in Chromium and WebKit. Committed, pending and serialized/reopened image-only, linked-file-only and mixed inputs retain the exact image encoding/size and linked path, a labelled attachment list and adjacent text. The reference placeholders show no preview or action when bytes are unavailable and fit the surface within 1 pixel of overflow. Reloading the fixture preserves the same metadata. The mapper and browser regressions hold this contract; agent-published file reads/saves remain covered separately by `published-files.mjs`.

## Compact send recovery (#914)

`refused-messages.mjs` renders the panel's real refusal notice over a store
holding two refused messages, the newer one sent again and taken. It checks
that the older one is offered with Send Again enabled, and that pressing it
sends the older message's own text and leaves no refusal behind. With
`?case=image` the older message carries an image: Send Again puts it back, and
the button stays held while the image waits, uploads, fails and waits again,
and goes live only once it is stored. Both it and
`send-failure.mjs` load a source fixture, so `run-all --mode prod` leaves them
out as dev-server only.

`send-failure.mjs` exercises the real conversation composer and overview reply
pill at 360px and 1000px in Chromium and WebKit. The failure is above the composer,
says **Error sending** with the actual code, keeps the full diagnostic off screen
and sends it unchanged to the injected clipboard port on Copy. Send Again reuses
the refused message; Discard removes it. Clipboard permission failures and panel
retry/unknown-admission identities are also covered by application/component tests.
Native first-use and refused-agent setup use the existing selected-agent readiness,
installer and authentication controls, without automatic message replay. Both
engines exercise download → sign-in → ready on first use; successful setup retires
the ordinary hint, while independent audit failures remain visible.
At 360px the setup card, with a production-sized **Download Claude · 117 MB**
beside **Retry**, keeps both buttons whole inside the card and the
card inside the viewport, within 1 pixel.
While installation waits, both setup and the downloads list display a labelled
indeterminate progress indicator and the known 117 MB archive size in Chromium and
WebKit. Neither supplies a numeric completion value; the indicator fits the 360px
viewport and disappears on completion without replaying installation or messages.
Its visible spinner animates with ordinary motion preferences and stops with
reduced motion.
The fixture’s `?clipboard=native` path uses the real host writer for isolated
native window Copy verification rather than the browser test capture.

## Pane placement ownership (#914)

`drag.mjs --only pane-placement-local` checks four real panes in both engines
and layouts: a nonzero column offset remains on its pane root, while all eight
placement properties have their independent initial values in conversation
content. Existing drag checks verify the resulting geometry and identities;
`perf-budget.mjs` measures the actual drop at 4× CPU.

## Remembered permission confirmation (#944)

`scripted-scenarios.mjs --remember --only permission,allow` clicks the published
Always allow in this project option through the real window and isolated gateway
in Chromium and WebKit. The browser must receive one successful action receipt
for its original request, the rule catalogue must contain one new allow rule,
and the provider continues with the scenario's allowed reply. The test revokes
its isolated rule before the next engine. Normal scripted permission checks also
require a confirmed receipt; eventual provider output alone cannot mask a
rejected acknowledgement. SDK ACP tests cover remembered allow and deny, exact
logical attribution and the mapped provider wire option. Service tests keep a
second review pending and require live publication after success or consumption
with a typed failure.
