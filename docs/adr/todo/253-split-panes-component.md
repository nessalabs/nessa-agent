# 253. Split panes are one module the workspace wraps

## Purpose

The desktop workspace's split panes — fractional layout, resize edges, drag
and drop with its zones, and a preview that draws every pane at the shape a
drop would give it — are moved out of the workspace into
`src/desktop/split-panes/`, a module with a small typed contract that any
desktop surface can wrap. It follows [238](../done/238-desktop-workspace-frontend.md),
which settled the behaviour; this record settles only where it lives and how
a host talks to it. Nothing a person sees changes.

- **Date:** 2026-09-28
- **Status:** proposed; implemented, in review

## Context

Everything about split panes is written into the workspace today. The drag
adapter (`workspace/adapters/dom/drag.ts`, about 1,400 lines) reads the
desktop store's workspace slice, dispatches workspace commands, builds a
carried session's card from sessions and transcripts, finds the side columns
by the workspace's dataset flags, and decides what each part of a pane keeps
to in a preview by workspace class names (`.workspace-dock`,
`.workspace-pane-header`). The pane grid, the FLIP flights and the resize
edges are tied the same way. Wrapping another surface in the same behaviour
would mean copying it, and a copy is a second owner of every rule in it
(gate 13).

What binds:

- **Behaviour must not move.** 238's drag table and its browser checks — 148
  in `drag.mjs`, with smoke, focus, responsive, safe-area and the 50ms frame
  budget in `perf-budget.mjs` — are the bar at every step.
- **The workspace keeps owning what a drop means for it.** A drop that folds
  the sidebar, drafts a pane leaves behind, and an agent's `movePane` or
  `dropSession` all run through the workspace's use cases. A component that
  handed back a new layout for the host to store would be a second owner of
  those rules.
- **A drag notices any change at once.** The drag ends the moment the store
  changes under it (238, `changed`); that is read synchronously today, and the
  `command-mid-drag` and `resize-mid-drag` checks depend on it.
- **The pane's root element is what moves.** FLIP and the preview transform the
  pane's root and counter-scale its direct children; a wrapper element around
  that root changes what is scaled.
- **The drag spans the host, not the grid.** Sessions are picked up from the
  session list and dropped on panes, and the side columns are never targets,
  so the drag listens on the host's root.

## Decision

`src/desktop/split-panes/` owns the pane model (`pane-layout`, `pane-sizing`,
`drop`, `drag`), the port a host implements (`application/ports.ts`), the DOM
adapters (drag and its preview, FLIP, pointer resize, tab order) and the UI
(`<SplitPanes>`, `ResizeEdge`, its stylesheet). The model speaks of panes
that each show an `item` — an opaque id the host gives meaning to — so
`Carried` is a pane or an item, and the room a drop may take from the host is
`spare` (`takesSpare`, not `foldSidebar`). A host supplies one
`SplitPanesSource`: it reads the layout now, subscribes to changes, names the
values whose change ends a drag (beside the panes' arrangement, which the
drag watches itself), says whether a carried item is still held and whether
panes can be aimed at, measures the room, and commits a drop, a resize, an
equalize or a fit — through its own commands. The preview asks the model's
`dropOutcome` of the layout the source reads, the same function the host's
commit uses. `<SplitPanes source renderPane empty>` renders the grid and
edges; `renderPane` receives each pane's placement, whether there are
several, and a `frame` of attributes and styles the host spreads on its own
pane root, so no element is added. `useSplitPanesDrag(root, source, options)`
is attached by the host at its root; its options build a carried item's copy,
name the elements that are never targets, and add attributes to strip from
copies. A pane's parts say what they keep to in a preview with
`data-split-keeps` (`top-left`, `foot`, `middle`; none is the top),
`data-split-through` marks a wrapper to look inside, and `data-split-scroll`
marks the scroller whose off-screen parts a copy leaves out. The workspace
keeps its side columns, window fit, session card, commands and stylesheet for
pane chrome, and wraps the module.

## Alternatives considered

- **A controlled component, `<SplitPanes layout onLayoutChange>`.** Simplest
  to read, and what most split-pane libraries offer. Lost: the workspace's
  drop runs rules the component cannot know (folding the sidebar, leaving
  drafts behind), so the host would either re-derive them from the new layout
  — a second owner — or ignore the component's layout. A host that has no
  such rules can still get this shape: its `commitDrop` applies the layout
  `dropOutcome` gives, as the drag previewed it.
- **Detecting changes by React render.** Lost: a change seen a render later
  moves when `command-mid-drag` and `resize-mid-drag` end the drag.
- **Moving it into nessa_ui.** Other apps would share it. Lost for now: the
  person asked that nessa_ui stay untouched, and the module still leans on
  desktop motion tokens and reduced-motion. A later move from one module is a
  smaller step than from the workspace.
- **Class names as the contract for pane parts.** Lost: they are the
  workspace's styling vocabulary, and a host would have to adopt it to get the
  preview right. Data attributes say what the component reads and nothing
  else.
- **Making `paneLimits` (four panes, 300 × 220, 8px gutter) a prop now.** Left
  for later: the gutter is also written in CSS, and changing that is a
  behaviour change this move must not carry.

## Implemented

The move landed in seven steps, each green (issue #253): the model and its
tests; the model's words; resizing and tab order; the drag driven through the
source, in place; the drag and FLIP moved; `<SplitPanes>` and its stylesheet;
these documents. The browser checks held at each step from the fourth on
(drag 148/148, smoke 28/28, focus 28/28, responsive 8/8, safe-area 90/90),
and computed styles and rects, sampled before and after in the states a drag
and a resize pass through, are unchanged but for the glow's custom
property's name.

Where the code refined the contract above, and why:

- **`fit()` takes no room.** The workspace's `fitPanes` measures the room as
  every other command that places a pane does; handing it a room the grid
  measured would be a second measure of the same thing.
- **Options that no host sets are not built.** No option words the announcer
  (`saying` is the module's), none renames the edges' labels, and there is no
  `applyDrop`: the one host, the workspace, needs none of them, and an option
  only one host sets is what _Consequences_ warns of. Each is a small
  addition when a second host asks for it.
- **What the copy's builder is given is `pressed`**, the element pressed —
  not `source`, which is the port's name.
- **The workspace's side lives in two files**: its source is a store adapter
  (`adapters/store/split-panes-source.ts`), and what it tells the drag of its
  page — the session's card, the side columns, the attributes to strip — is
  one DOM adapter (`adapters/dom/split-panes-drag.ts`). The window builds one
  source, which the grid and the drag share.
- **FLIP moved with the drag**, not before it: it lets go of the drag's
  preview, so moving it first would have had the module import the workspace.
- **The grid places the pane**: `split-panes.css` places a pane's root by the
  frame's properties (`.split-panes-grid > [data-pane-key]`), and a copy of a
  pane in the carried layer by `.split-panes-ghost-pane`; the pane's look
  stays the host's. The announcer's hidden style was only the drag's, so it
  moved too (`.split-panes-announcer`).
- **The boundary is checked**: `pnpm architecture` refuses any import into
  `split-panes/` but its barrel and — for a host's model and use cases, which
  may not import React — its pure model files.

## Consequences

- Another desktop surface gets split panes by implementing one source and
  spreading one `frame`, with the drag zones, holds, preview and flights as
  they are.
- The rules have one home. A change to a drag rule is made once, and 238's
  table stays the contract, its paths updated.
- Harder: the contract is a real API now. Adding a behaviour means deciding
  whether it belongs to every host (the module) or to one (an option), and a
  change to an option's meaning touches every host.
- The rename (`sessionId` → `item`, `session` → `item`, `foldSidebar` →
  `takesSpare`, `.workspace-drag-*` → `.split-panes-*`) touches the workspace's
  use cases, tests, selectors and the browser checks' selectors in the same
  change, with no aliases left behind.
- Watch for: an option that only the workspace ever sets growing into a
  second contract, or the workspace reaching past the source into the
  module's internals — either says the boundary is in the wrong place.
