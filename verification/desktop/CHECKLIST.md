# Desktop verification checklist

What we always test and reproduce on the desktop window. Each item names the
contract it holds (and where that contract is written), how to check it — a
script under [`scripts/`](scripts/) or manual steps — and the noise known to
be harmless. Why these exist and when to run them:
[Browser verification for UI](../../CODING_STANDARDS.md#browser-verification-for-ui).
How to run them: [`verification/README.md`](../README.md).

ADR 238 is [`docs/adr/todo/238-desktop-workspace-frontend.md`](../../docs/adr/todo/238-desktop-workspace-frontend.md);
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
  pane dragged across zones (dropped, and cancelled), and the Agents overview
  opened, walked, answered and left.
  _Check:_ `perf-budget.mjs` (defaults: `--mode prod --throttle 4 --runs 3`).
  Read max and median per row; every over-budget frame carries its Long
  Animation Frame attribution (scripts, forced layout, style-and-layout time).
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
- _Harmless (exempt by design):_ titlebar rows themselves, tooltips, the drag
  copy and the hidden title sizer. Decorative art is **not** exempt: the
  picture band (`[data-sliver]`) and the night scene begin after the controls
  in the corner pane and wait out of sight while panes travel.

## Drag and drop

_ADR 238 › Drag and drop_.

- [ ] **The copy follows the pointer one to one** — its corner is the pointer
  less the grab offset, no pull toward a target. _Check:_ `drag.mjs` (`sweep-across-zones`).
- [ ] **Every preview stays inside the grid.** _Check:_ `drag.mjs` (`sweep-across-zones`).
- [ ] **Panes move one way between zone changes** (no back-and-forth within a
  zone). _Check:_ `drag.mjs` (`sweep-across-zones`).
- [ ] **Zones are direction-aware**: sweeping sideways near a tall, narrow
  pane's top reaches its side, never above or below. _Check:_ `drag.mjs` (`sideways-top`).
- [ ] **A zone holds at its boundary** (another must win by 12px): ±6px
  jitter does not flicker it. _Check:_ `drag.mjs` (`boundary-jitter`).
- [ ] **Escape, or a drop outside the window, cancels**: the copy flies home,
  the layout is unchanged, no pane is left transformed.
  _Check:_ `drag.mjs` (`escape-cancels`, `outside-cancels`). If an engine
  delivers no pointer event outside the viewport, the script says it could
  not run — check that drop by hand in the app.
- [ ] **No text selection is left behind**, during or after a drag (WebKit
  selected transcript text before). _Check:_ `drag.mjs`.
- [ ] **Preview equals commit** — the placeholder marks exactly the rect the
  drop takes. _Check:_ unit test `panes.test.ts`; by eye with `--headed`.
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
- [ ] **Both layouts answer the same keys with the same parts.** _Check:_
  unit test `layouts.test.tsx`; every script runs both layouts.
- _Harmless:_ in dev, StrictMode runs effects twice; if focus looks wrong only
  in dev, confirm with `--mode prod` before reporting.

## Agents overview

_ADR 238 › What fills the content region_ (the overview is workspace state).

- [ ] **⌘0 and the sidebar's Agents entry open it; asked again, it stays.**
  _Check:_ `focus.mjs` (`focus-overview`), `smoke.mjs` (`overview`).
- [ ] **The keyboard walks its items; ⌘↩ allows, ⌘⌫ denies, ⌘R replies**, and
  the caret stays in the overview after an answer. _Check:_ `focus.mjs`
  (walk); `perf-budget.mjs --only overview-answer` (answer lands); ⌘⌫ and ⌘R by hand.
- [ ] **Escape, or anywhere else chosen, goes back to the panes** with the
  caret in the focused pane's composer. _Check:_ `focus.mjs` (`focus-overview`).
- [ ] **The sidebar marks what is shown**: Agents while the overview is, a
  channel or session only while the panes are. _Check:_ manual.

## Composer and approval card

- [ ] **The approval card arranges itself by its own width** at 280, 340,
  420, 600 and 900 px, with a short and a very long command: no word of the
  command broken, no button label wrapped, nothing overflowing.
  _ADR 238 › Decision_ (`ui/`, `approval-request.tsx`).
  _Check:_ `responsive.mjs --only approval-card --shots <dir>`, then look at the shots.
- [ ] **The model is shown once, in the composer** — not in the pane header
  or the transcript heading. _Check:_ manual (and in shots from `responsive.mjs`).
- [ ] **Composer controls never overlap**, down to the compact form.
  _Check:_ `responsive.mjs --only composer-chips`.
- [ ] **Send from a new session arrives in its transcript.** _Check:_ `smoke.mjs` (`send`).

## Settings

_ADR 238 › Decision_ (Settings is a typed catalogue; modal; its sidebar folds below a page's 420px).

- [ ] **Settings opens and leaves**; the window under it is inert while open.
  _Check:_ `smoke.mjs` (`settings`); inertness by hand (click under it).
- [ ] **Its sidebar folds for room below a 420px page.** _Check:_
  `responsive.mjs --only settings-fold`.
- [ ] **Pending settings say "Not available yet" and their controls are
  disabled** — nothing looks as if it works when it does not. _Check:_ unit
  test `settings-view.test.tsx`; by eye.

## Menus and tooltips

_Standing design preferences_ (the user's; hold them in review):

- [ ] **No ring or border on a selected row or a highlighted menu item** —
  the highlight is a fill. _Check:_ manual, in both themes; screenshot a
  highlighted item.
- [ ] **No leading icons in a menu unless every item has one.** _Check:_ manual.
- [ ] **Shortcuts are right-aligned** in menus and tooltips, and named only
  where they work (`shortcuts.ts` labels them). _Check:_ manual.
- [ ] **A pane's actions are "…" then "×", shown on hover.** _Check:_ manual (hover a pane header).
- [ ] **A submenu opens beside its item, fully visible, not clipped by an
  ancestor**, and takes the pointer. _Check:_ manual near the window's right edge.
- [ ] **One tooltip for the whole window, in the window's glass.**
  _ADR 238 › Decision_ (`use-window-tooltips.ts`). _Check:_ manual.

## Motion and reduced motion

_ADR 238 › Decision_, last paragraph ("Motion animates transform and opacity only").

- [ ] **Only transform and opacity animate**; layout changes are applied at
  once and played back with FLIP. _Check:_ DevTools › Performance: no
  layout-inducing properties in the animation; `perf-budget.mjs` attribution
  shows forced layout per script.
- [ ] **Calm, no jitter**: nothing moves one way then the other within a
  transition. _Check:_ `drag.mjs` (`one-way`); by eye with `--headed` and a
  slowed animation for the rest.
- [ ] **Reduced motion sets durations to zero** (Settings › Motion, or the
  system's), and script motion follows. _Check:_ `safe-area.mjs --reduced-motion`
  and by hand; with less motion, a drag moves only the copy.

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

## Console errors

- [ ] **No console error, page error or failed request** while any script
  runs. _Check:_ every script adds them to its failures; `smoke.mjs` reports them
  per layout as `console`.
- _Harmless:_ the first page in a fresh browser asks for `/favicon.ico` and
  gets a 404 (matched by source URL in `lib/selectors.mjs`, `harmlessConsole`;
  kept in the JSON as `harmless`). Vite's `[vite] connecting…` / HMR
  messages are logs, not errors. A reload caused by another edit landing on
  the dev server mid-run is not a finding — re-run.
