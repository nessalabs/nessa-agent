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
- [ ] **The budget decides on the unrounded frame** (#369). The table rounds
  maxima for display. A 50.1 ms frame fails when that table shows 50; an exact
  50 ms frame and a 49.6 ms frame pass.
  _Check:_ `lib/perf.test.mjs` (`frame budget`).
- [ ] **Style-and-layout time is a duration on the performance clock** (#369). The
  sample shifts `start`, `renderStart`, and `styleAndLayoutStart` together
  onto the interaction. A zero phase start means that phase did not run, so
  the diagnostic is absent rather than a negative duration.
  _Check:_ `lib/perf.test.mjs` (`long animation frame clock`).
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
- [ ] **Column titles sit inline after the controls when they fit, below when
  they do not**, and an inline title never overlaps a titlebar control.
  _ADR 238 › The titlebar's safe area_ (`titlePlacement`).
  _Check:_ `responsive.mjs --only column-title`; drag a column edge slowly
  across the flip point by hand and watch for a flicker (the 12px hold).
- [ ] **A column's titlebar action hides at once as it folds, shows once it
  has slid in; a pane's header hides while it flies.** _Check:_ manual, with
  `--headed` and a slowed animation (DevTools › Animations at 10%).
- _Harmless (exempt by design):_ titlebar rows themselves, tooltips and the
  hidden title sizer. Decorative art is **not** exempt: the picture band
  (`[data-sliver]`) and the night scene begin after the controls in the corner
  pane and wait out of sight while panes travel. Nor is the drag's copy: its
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
- [ ] **Nothing carried is painted under the window's controls**, the corner
  pane's copy included, every frame, and the corner pane's copy lays its
  header out as the pane does, its title as far in (± 2 px). _Check:_
  `drag.mjs` (`copy-under-controls`).
- [ ] **No text selection is left behind**, during or after a drag. _Check:_
  `drag.mjs` (`sweep-across-zones`, `outside-cancels`).
- [ ] **Preview equals commit** — the placeholder marks exactly the rect the
  drop takes. _Check:_ unit test `panes.test.ts`; by eye with `--headed`.
- [ ] **The copy takes the shape of where it would land** (ADR 238 › _What
  the copy and the panes are drawn at_): with a zone shown and the pointer
  at rest, the copy's painted size is the placeholder's (± 2 px) and its
  centre is on the pointer (± 2 px); with no zone it is the carried pane's
  own size; each change of size runs one way, never past where it goes,
  with nothing painted under the controls and no title drawn stretched.
  _Check:_ `drag.mjs` (`copy-takes-slot-shape`; `--shots <dir>` saves it
  below, beside, and over its own place); unit tests `split-panes/model/drag.test.ts`,
  `split-panes/adapters/dom/drag.test.tsx`,
  `workspace/adapters/dom/split-panes-drag.test.tsx`.
- [ ] **Every pane previews the shape it lands at**: with a zone shown and
  the pointer at rest, each pane is painted at the rect the drop then gives
  it (± 2 px), its conversation centred across that shape (± 2 px), its
  header held to the top left, and cut to it —
  its title never drawn stretched, any frame — and a drag leaves no pane at a size of its own. _Check:_
  `drag.mjs` (`preview-panes-take-shape`; every drag check's residue).
- _Harmless, and not a failure:_ a single read of a title's transforms in
  WebKit that mixes two moments. How the two stretch checks read a title is
  `recordShapes`' (`drag.mjs`), and why, with the runs and probes, is #365.
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

## Keyboard and focus

_ADR 238 › Focus follows the focused pane_; keys in
`src/desktop/workspace/ui/layouts/shortcuts.ts`.

- [ ] **The caret lands in the focused pane's composer** after ⌘N, ⇧⌘N,
  ⌘\\ and a pick, ⇧⌘\\, ⌘1–4, ⇧⌘] and ⇧⌘[, and ⌘W. _Check:_ `focus.mjs` (`focus-panes`).
- [ ] **Closing ⌘K without a pick, or Settings, gives focus back** to what
  opened it. _Check:_ `focus.mjs` (`focus-panes`).
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

_ADR 238 › What fills the content region_ (the overview is workspace state).

- [ ] **It is always offered**: on a fresh profile, with nothing stored, the
  sidebar shows "Agents" (and no separate Needs you or Running rows) and ⌘0
  opens it. _Check:_ `smoke.mjs` (`overview`); unit test `layouts.test.tsx`.
- [ ] **⌘0 and the sidebar's Agents entry open it; asked again, it stays.**
  _Check:_ `focus.mjs` (`focus-overview`), `smoke.mjs` (`overview`).
- [ ] **The keyboard walks its items; ⌘↩ allows, ⌘⌫ denies, ⌘R replies**, and
  the caret stays in the overview after an answer. _Check:_ `focus.mjs`
  (`focus-overview`, the walk); `perf-budget.mjs --only overview-answer` (answer lands); ⌘⌫ and ⌘R by hand.
- [ ] **One press answers one request** (_ADR 238 › What fills the content
  region_): a held ⌘↩ answers one; two presses 80ms apart answer one; a held ↩
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

_ADR 326_ ([`docs/adr/todo/326-widgets.md`](../../docs/adr/todo/326-widgets.md)):
a plugin's view is drawn inline in a message, in a pane of its own, or in the
window over the panes; Escape and focus as its _Focus_ and _Escape_ say. The
scripts drive the sample plugin the sample workspace registers
(`src/desktop/widgets/fixture/`), on its session "Widget hosts, every state".

- [ ] **A card opens its widget in a pane beside its conversation, and the
  window over the panes, and Escape goes back** — the pane's rect beside the
  conversation's, the caret in its body; the window inside the chat area and
  clear of the session list, the caret in its body; the panes as they were
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

## MCP Apps

_ADR 344_ ([`docs/adr/todo/344-mcp-ui.md`](../../docs/adr/todo/344-mcp-ui.md)),
#349: an MCP server's app is drawn behind a sandbox proxy on another origin,
inline, in a pane (its fullscreen) and in the window, and spoken to over the
`ui/*` bridge. The script drives the fixture app the sample workspace
registers (`src/desktop/widgets/app/fixture/`), on its session "An MCP App,
in its sandbox". Every row of the bridge's design table is a jsdom test
(`app/application/bridge.test.ts`, `app/ui/app-view.test.tsx`).

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
  held back, going back across a move to a fragment (on a page of its own,
  where an answer to an earlier check, coming after a later load, ends no
  wait), an app forging departures, and a third party forging them and the
  check's answers at every proxy and app frame, are not; a deadline no
  timer can wait loads nothing. _#349 design, L32._ _Check:_ `mcp-apps.mjs --only
  departures,departures-back` (dev server: it imports the host's builder; it
  waits past the initialize deadline).
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
- [ ] **One tool call is drawn once** (#418): a harness reports one call as an
  announcement and then updates under its id (Codex three frames, or two
  in the recorded `review_rows` turn; Claude four), and the window draws it as one transcript step and one inline app
  frame, so the app mounts once. _#418 design._ _Check:_
  `mcp-apps-gateway.mjs --agent codex --scripted` and `--agent claude
  --scripted`, `renders`: its step and frame counts (no model, no sign-in: the
  scripted agent replays the recorded frames under the real gateway).

## Composer and approval card

- [ ] **The approval card arranges itself by its own width** at 280, 340,
  420, 600 and 900 px, with a short and a very long command: no word of the
  command broken, no button label wrapped, nothing overflowing.
  _ADR 238 › Decision_ (`ui/`, `approval-request.tsx`).
  _Check:_ `responsive.mjs --only approval-card --shots <dir>`, then look at the shots.
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
  _Check:_ `app-review.mjs --shots <dir>`.
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
- [ ] **Pending settings say "Not available yet" and their controls are
  disabled** — nothing looks as if it works when it does not. _Check:_ unit
  test `settings-view.test.tsx`; by eye.
- [ ] **A setting for one layout says so elsewhere**: "Show session list" and
  "Keep running sessions at the top" work in three columns; in sessions in
  the sidebar and classic their switches are disabled and say "Three columns
  only". _Check:_ unit test `settings-view.test.tsx` (per layout).
- [ ] **Advanced › Experimental is the home of previews**: Advanced sits just
  before About with its flask in both icon families; its one tab,
  Experimental, shows "Nothing to try right now." and no control while no
  preview is on offer; General has no Experimental tab; search finds it as
  "advanced", "experimental", "labs" or "preview". _Check:_ `smoke.mjs`
  (`settings`); unit tests `settings-view.test.tsx`,
  `settings-catalogue.test.ts`, `icon-provider.test.tsx`; the flask by eye in
  both families.

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
  in a new conversation, its app frame renders the chart, once. _Check:_
  `mcp-servers-gateway.mjs --only done-when` (needs the agent signed in on the
  machine). "Once" depends on #418's fix (#421) being in the tree.

## Menus and tooltips

_ADR 238 › Interaction and visual rules_ holds the rules; check each by hand,
in both themes, and screenshot what a rule is about (a highlighted item, a
menu near the window's right edge, a pane header on hover).

- [ ] **Every rule in _Interaction and visual rules_ holds.** _Check:_ manual.

## Motion and reduced motion

_ADR 238 › Decision_, last paragraph ("Motion animates transform and opacity only").

- [ ] **Only transform and opacity animate**; layout changes are applied at
  once and played back with FLIP. _Check:_ DevTools › Performance: no
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
  `nessa-panel`, principal `surface:nessa-panel`. No console error, page error
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
  window.** _By hand:_ `pnpm app` against a gateway with
  `scripts/mcp-test-server` configured. This row is checked by hand.
  `gateway-window.mjs` has no step for it; adding one is #574.
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

- [ ] **No console error, page error or failed request** while any script
  runs. _Check:_ every script adds them to its failures; `smoke.mjs` reports them
  per layout as `console`.
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
for all six engine/scenario pairs. This verifies browser interactions with the
production panel; it does not exercise an operating-system file picker.


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
local notices remain.

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
  no idle transcript reads. `message-sync.mjs` measures these contracts over the
  production source, store and window with controlled gateway replies. See the
  [ordering table](../../docs/reviews/startup-latency.md#desktop-transcript-delivery-experiment-532).
  Three fresh-page runs report delivery median/max and frame attribution. Chromium
  uses calibrated 4x CPU throttling and checks the 50 ms frame budget through
  `lib/perf.mjs`; WebKit delivery is reported separately without CPU throttling.
  DOM plus frame opportunities do not measure transport, provider startup or compositor paint.
