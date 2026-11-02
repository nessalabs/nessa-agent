# 1232. A full view keeps the corner and the titlebar for the window

## Purpose

`columns.mjs` still checked an older window: every full view put "nessa
Studio" where Agents puts it, and the view itself took the press at the top
of the window. Devices no longer does either, on purpose. The checks should
say what the window does now.

- **Date:** 2026-11-02
- **Status:** accepted

## Context

Two checks fail on main (`dd69f80eb`), in the columns layout and the sidebar
layout, at 1440×900 in Chromium:

- `rail-corner` — "nessa Studio" is at `[null]` in a full view and at a real
  box in Agents.
- `rail-view-inert` — "plugin view exposes the workspace cards or controls
  above it."

The keys under Devices already do nothing. Sidebar, list, and overview come
back to Agents as they were left. The failure is only the hit at the top of
the window.

Bisect, same command, Chromium, both layouts, 1440×900:

| Commit | `rail-corner` | `rail-view-inert` |
| --- | --- | --- |
| `56c3c475a` Devices hub, the parent | held | held |
| `25a86763c` Devices leaves the corner clear of "nessa Studio" | failed | held |
| `6a9e4ec3b` the full view's titlebar moves and zooms the window | failed | failed |
| `dd69f80eb` main, including the rail pill (#1231) | failed | failed |

Each failing commit says why, and the stress step `devices` already requires
the new corner and the drag region. #1231 moves the words 12px right while
the rail is open (`translateX(calc(var(--rail-pill-pad) + 8px))`) and does
not put "nessa Studio" back on Devices. The toggle stays in one box. This
record does not change that CSS.

## What the checks assert

`rail-corner` — the toggle is drawn in one bottom-left box, rail open or
closed, sidebar shown or hidden. With the rail closed and a visible footer
mark, the toggle sits in that slot. "nessa Studio", when it is drawn, starts
at or after the toggle's right edge, and Studio's square is not drawn beside
the toggle. Devices draws no "nessa Studio". Coming back to Agents puts the
words back in the box that rail state had. The open-rail box and the
closed-rail box are not required to match.

`rail-view-inert` — under Devices, ⌘B, ⌘0 and ⌥⌘S change nothing behind the
view, and Agents finds the workspace as it was left. The view's top is the
window's top. A hit just below the titlebar is inside the view, not a
workspace card. The titlebar's free room is a drag region, and its one
control folds and reopens the rail.

## States

Measured in Chromium at 1440×900 on `dd69f80eb`. The toggle was
`[18, 851, 28, 28]` in every row. "nessa Studio" in Agents was
`[58, 851, 87, 28]` with the rail open and `[46, 851, 87, 28]` with it
closed (12px, the pill's step). In the sidebar layout with the sidebar
hidden, the panes are alone and the words are not drawn.

| View | Rail | What holds the corner | "nessa Studio" | Titlebar free room | ⌘B, ⌘0, ⌥⌘S |
| --- | --- | --- | --- | --- | --- |
| Agents | open | the sidebar, or the session list when the sidebar is hidden | drawn, clear of the toggle, stepped past the pill | the pane header under it takes the press | the workspace answers |
| Agents | closed | the same column | drawn in the mark's gap, at the toggle's right edge | the same | the workspace answers |
| Agents | either | the panes alone (sidebar layout, sidebar hidden) | not drawn; the words never cover a chat | the same | the workspace answers |
| Devices | open | the view's cards, under the corner | not drawn | a `data-tauri-drag-region` | nothing behind the view changes |
| Devices | closed | the same cards | not drawn | a `data-tauri-drag-region` | nothing behind the view changes |

The toggle is the same box in every row, including Devices.

## Invariants

| Guarantee | Enforcer |
| --- | --- |
| The toggle's box does not change when the rail, the sidebar, or the view changes | `rail-corner`. The toggle is positioned once in `side-rail.css` |
| With the rail closed, the toggle sits in the visible footer mark slot | `rail-corner` (`markSlot`) |
| Drawn "nessa Studio" starts at or after the toggle's right edge. The open-rail and closed-rail boxes may differ by the pill's step | `rail-corner`. The step is `side-rail.css` under `[data-rail="open"]`, checked as a gap by the stress step `rail` |
| Studio's square is not drawn beside the toggle while the rail is offered | `rail-corner` (`squareShown`). The footer renders no mark when the rail is on |
| Devices draws no "nessa Studio" | `rail-corner`. The layout omits `StudioCorner` when `studio === false`. Devices sets that flag (`side-rail.test.tsx`). The stress step `devices` counts `.side-rail-studio` |
| Agents gets its own box back after Devices | `rail-corner`, compared within that rail and sidebar state |
| Under Devices, ⌘B, ⌘0 and ⌥⌘S leave the sidebar, the list, and the overview as they were, and Agents finds them so | `rail-view-inert`. The workspace is `inert` while a rail item is shown |
| The Devices titlebar's only control folds and reopens the rail | `rail-view-inert` |
| The view starts at the window's top. Just below the titlebar the hit is inside the view. The titlebar's free room hits a drag region | `rail-view-inert`. The view is `inset: 0`. Under `[data-rail-view]` the titlebar sits above it and its free room has `pointer-events: auto` |

## Alternatives considered

- **Put "nessa Studio" back on Devices, and let the view take the press in
  the titlebar.** That is what the old checks require. Devices' cards run
  under the corner, and a drag on the Devices titlebar has to move the
  window. The stress step `devices` already fails if either comes back.
- **Drop the full-view lines and the top-of-window hit.** The run would go
  green, and a view that drew the words over the cards, a titlebar that
  brought back the Agents controls, or a drag that fell through to a pane
  would pass. That weakens the check.
- **Leave it to the stress step and delete these two checks.** The stress
  step needs a signed-in gateway and is not in the desktop handoff run.
  These checks are how the window is held without one.
- **Pin the 12px step in `rail-corner`.** The stress step `rail` already
  owns that gap. A second copy here would fight the pill the next time its
  pad changes. This check only requires the words to stay clear of the
  toggle, and not to treat the open box and the closed box as one.
- **Keep asserting Notes, Calendar, Browser and Files.** They are not rail
  items. The only full view is Devices.

## Consequences

The columns checks agree with the window and with the stress step. They do
not move the rail's CSS, so the pill in #1231 stays as it is. A later full
view that wants "nessa Studio" sets `studio` other than `false` and draws
the corner; `rail-corner` would then be wrong for that item until it learns
the flag. Devices is the only full view today.

## Review brief

- **Base:** `dd69f80eb` (`origin/main`, which includes #1231).
- **Head:** `cursor/rail-full-view-contract-cb04`. Review the committed tree.
- **Intended behavior:** the state table above. The product already did this.
  The checks, the checklist, and the architecture paragraph now say it.
- **Scope:** `verification/desktop/scripts/columns.mjs`,
  `verification/desktop/CHECKLIST.md`, the side-rail sentences in
  `docs/ARCHITECTURE.md`, and this record. Not `side-rail.css`. Not the
  `claude/*` branch.
- **Look for:** a check that requires the open-rail and closed-rail "nessa
  Studio" boxes to be the same (that fights #1231); a deleted assertion with
  nothing in its place; a second copy of the pill's 12px gap.
- **Handoff evidence:** below. No state chart owns this corner. The
  right-panel chart only says a full view hides the panel, which this
  leaves as it is, so that page's `sourceRevision` stays.

## Evidence

Dev server, on the check change in this record:

```sh
node verification/desktop/scripts/columns.mjs --engine chromium,webkit --sizes 1440x900,1000x700,760x700
```

`rail-corner` and `rail-view-inert` held in both engines, both layouts, and
all three sizes. The script: 256 held, 6 not run (`waiting-rows` has no
session list in the sidebar layout), 15 failed. Every failure is a WebKit
frame count in `quiet-composer`, `sidebar-motion`, or `split-motion`. The
same three fail on `dd69f80eb` with this change stashed. Chromium held those
three at every size and layout.

Revert probe, Chromium, columns layout, 1440×900, then restored:

- Without `studio: false`, `rail-corner` fails: Devices draws "nessa Studio"
  at the Agents box, open and closed, sidebar shown and hidden.
- With the titlebar free room at `pointer-events: none`, `rail-view-inert`
  fails: the full view's titlebar does not move the window.

`side-rail.css` and `side-rail.tsx` are not in the diff.
