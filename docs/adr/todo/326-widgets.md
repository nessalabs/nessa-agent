# 326. A plugin's view is a widget the window hosts, in a message, in a pane, or over the panes

## Purpose

Some of what a conversation produces is better seen than read: an experiment
a swarm is running ([333](333-experiments.md)), the subagents a conversation
has put to work ([329](329-subagents.md)), and later whatever else a plugin
draws. This record settles how the desktop window hosts such a view — where it
can appear, how a person moves it between those places and back, and what is
drawn when it cannot be — without the window knowing any plugin, and without
any plugin reaching into the workspace.

- **Date:** 2026-09-30
- **Status:** accepted

## Context

A prototype (`exp-prototype` @ `5bfaa225`, `src/desktop/widgets/`) showed the
flow people liked: a card in the assistant's message, a column beside the
conversation, the widget over its whole pane with a breadcrumb, a pane of its
own like a chat's, and Escape back. Its structure is what cannot ship:

- **The host imports its plugins.** `widgets/ui/registry.tsx` imports the
  experiments and subagents views to build a static table, so the generic host
  depends on every plugin, and a new plugin edits the host.
- **A widget pane borrows the session-id slot.** A pane showing a widget holds
  `widget:<plugin>:<id>` where a session id goes
  (`workspace/model/pane-item.ts`). Every reader of a pane's item — what is
  showable, on screen, kept as a draft, the header, the drag's card — has to
  know the string might not be a session; and a session id that began `widget:`,
  or a widget id with a `:` in it, would collide with another item, which split
  panes treat as the same pane (`split-panes/model/pane-layout.ts`,
  `paneShowing`).
- **The workspace imports a plugin to draw its header.** The pane header draws
  the conversation's subagents from the subagents vertical, which itself reads
  the workspace's transcript: a cycle.
- **Nothing says what happens when a widget cannot be drawn,** or where
  Escape goes when a widget, a menu, a drag and the overview all want it.

What binds:

- **Split panes stay host-agnostic.** A pane shows an opaque `item` string
  ([253](../done/253-split-panes-component.md)); its meaning is the host's.
- **A plugin owns its data.** An experiment is read from `ExperimentSource`,
  subagents from `SubagentSource`; the widget layer is not a second path to
  either, and some of those sources will read over the wire, so a widget can
  be not read yet as well as gone.
- **The transcript is a projection of the source.** A widget reaches a message
  as one of its parts, like text and steps.
- **Escape already has owners** in 238: menus and dialogs, the drag (it
  cancels and goes no further), the overview, the edge peek.
- **The content region already has a second view.** The Agents overview (⌘0) is
  drawn in a layer over the session list and the panes, which stay as they were
  beneath it; 238 says how it is left: Escape, or going anywhere else — a
  session chosen, any change of the panes — is going back to them
  (`ContentView`, `usecases/navigation.ts`, `goesSomewhere` and `changesPanes`
  in the slice).

## Decision

### The contract

`src/desktop/widgets/` owns the contract and the hosts, and knows no plugin.

- A **`WidgetRef`** is `{ plugin, id }`: the plugin that draws it and the
  plugin's own id for the thing drawn. It is the one shape everywhere — in a
  pane item, in a transcript part, in a command.
- A plugin is one of **two kinds** behind the same host, places and states:
  `native` — in-process React the window trusts (subagents, 329) — and `app` —
  an MCP App, HTML rendered in a sandboxed iframe and spoken to over the `ui/*`
  bridge ([344](344-mcp-ui.md)). The host picks the renderer by kind; nothing
  else about a widget depends on it.
- A **native plugin** is `{ id, name, useWidget(id), views, SessionAccessory? }`
  — `name` is what the window calls it while a widget is not read yet
  ("Experiment"). `useWidget` is the plugin's hook answering, reactively, `{
  kind: "ready", title, origin? } | { kind: "unread" } | { kind: "missing" } | {
  kind: "off" } | { kind: "unshowable" }` — `origin` is the session the widget
  belongs to, when it has one; `off` says the plugin's preview is turned off;
  `unshowable` says the plugin holds the widget but cannot draw it (an
  experiment that failed validation, 333). `views` offers `pane`, and any of
  `inline` and `window`. A `SessionAccessory` is something the plugin draws in a
  session pane's header, given the session's id and `openWidget` alone — it has
  no widget of its own to open, close or step back in, and no place; what it
  opens goes beside that session's pane (the subagents' avatar stack, which
  opens their panel), so the workspace draws it without importing the plugin.
  Each plugin's hook is its own, so a host keys the view it draws by plugin.
- Views are given only the widget's id, the `place` they are drawn in, and the
  **host's callbacks**: `open(place)`, `close()`, `openWidget(ref, place)` (a
  plugin opening another plugin's widget — an experiment's agent opening the
  subagents panel; its pane goes beside the pane showing the *caller's* `origin`
  — for a `SessionAccessory`, the header's own session — which the caller's host
  holds, since only a plugin's own hook knows its widget's origin), and
  `onEscape(handler)` (below), with a read-only **host context** — theme,
  locale, the place's size and safe area. These are shaped after MCP Apps'
  bridge so the `app` kind is a translation, not a second contract:
  `open(place)` is `ui/request-display-mode`, the host context is `hostContext`.
  Sending a message and updating the model's context (`ui/message`,
  `ui/update-model-context`) were built with the `app` host as an app's own
  port (344, #349); a native view has not needed them, and gets them on
  `WidgetHost` when one does.
- Native plugins are **registered once, in composition** (`main.tsx`), into a
  `WidgetRegistry` provided to the tree; `app` plugins are **registered and
  unregistered while the window runs**, as the gateway reports MCP servers with
  UI (344). Two plugins with one id is an error either way — a composition error
  at start, a typed refusal at run time — tested for both. The host looks a
  plugin up by id through the registry and nothing else.

### Places

| Place | What it is | Opened by | Left by |
| --- | --- | --- | --- |
| `inline` | a card in the message the widget part is in | the transcript | — |
| `pane` | a pane of its own in the split grid, as a chat has | `open("pane")`: beside the pane showing its `origin`, by the workspace's `openBeside` and its rules, or in the focused pane's place when there is none | the pane's close or ⌘W, as any pane; the last pane, as 238 has it, goes back to a new session's home, where a new session goes (`createDraft`: the channel being looked at, else the first) — `closePane` gains that case, which today leaves a last pane not showing a session as it is |
| `window` | the content region's third view, beside the panes and the overview: the widget drawn instead of the panes, which stay beneath it as they were | `open("window")` | as the overview is — Escape or its close, a session chosen, or any change of the panes goes back to the panes; ⌘0 goes to the overview — with two differences: it covers the content region only, so the session list stays in reach beside it; and ⌘W closes the window, never a pane beneath it (over the overview, ⌘W closes the focused pane, 238) |

The sidebar beside the window keeps the channel and the focused session marked
as chosen, as over the panes; only the overview takes the sidebar's choice for
itself. A session row dragged over the window finds no target and a release
changes nothing, as over the overview (238's drag table).

The content view is `panes | agents | { widget }`, one at a time, and the window
place is its third value: nothing is moved into or out of the grid to show a
widget over it, so a widget can be in a pane and in the window at once, as a
session can be in a pane and in the overview's peek. `openWidget` from the
window replaces the widget shown there. A widget pane moves, resizes and closes
as any pane does; nothing carries a widget into the grid from outside it. A
widget with an `origin` offers a way back to it — its breadcrumb's first step —
which focuses the pane showing that conversation, or opens it beside the
widget's pane; from the window, it goes back to the panes and focuses the pane
showing it, or opens it in the focused pane. A widget whose `origin` is removed
stays where it is, without a way back.

**Focus.** 238 has one rule for where the caret goes
(`workspace/adapters/dom/focus.ts`), in two halves: when a pane takes focus, and
when whatever held the caret goes away, it lands in the focused pane's composer.
This record amends the whole rule, both halves: the caret lands in the focused
pane's composer when it shows a session, and in its body when it shows a widget,
for the view to place further; and while the content view is a widget, focus
that falls away lands in the window's widget body, not a pane beneath it — so a
run's detail closing, or a subagent list giving way to one subagent, leaves
focus in the widget's pane, or in the window's widget, where the next Escape
still reaches it. Opening a widget in a pane is a pane taking focus; opening one
in the window moves focus into it; going back to the panes is the focused pane
taking focus.

**Escape**, after 238's owners (a menu or dialog, a carrying drag, the edge
peek, the open overview), goes to the widget in front — the window's while it
shows one, else a pane's while focus is inside that pane. The view has the first
refusal (`onEscape`, for a view with somewhere to step back to); then the host,
which goes back to the panes from the window. A widget in a pane is not closed
by Escape, as a session pane is not. The cases, in order — each owner takes
Escape and stops it, so none after it answers:

| Escape arrives while | Who takes it | How it holds the rest off |
| --- | --- | --- |
| focus is in a menu or a dialog | the menu or dialog | `defaultPrevented`; every later owner also leaves a target inside one alone (`adapters/modal.ts`) |
| Settings is open | Settings | the window under it is inert |
| a drag is carrying | the drag, which cancels | stops it in capture (`split-panes/adapters/dom/drag.ts`) |
| the edge peek is shown | the peek, which dismisses and nothing more | heard on the document in capture — after the window's, where the drag is — marked handled and stopped (`adapters/use-edge-peek.ts`); in Settings, itself a dialog, its own peek too |
| the overview is open | the overview | the content view is the overview |
| the session list's search holds a query | the search, which clears it | marked handled; the next Escape goes on |
| the window shows a widget | its view's last step back, else the host: back to the panes | from anywhere outside the above — the session list's rows, and its search once empty, included (the window covers the content region only) |
| focus is inside a widget's pane | its view's last step back, else nothing | — |
| anything else | nothing here | — |

`workspace/adapters/dom/widget-escape.ts` answers the last four rows, and each
row is a test (`widget-escape.test.tsx`, `use-edge-peek.test.tsx`,
`ui/layouts/widgets.test.tsx`) and a check in `widgets.mjs`.

The window's **layout is not persisted** (238), and widgets do not change that.

### In the workspace

A pane's item is a **tagged union** — `{ kind: "session", sessionId } | { kind:
"widget", widget: WidgetRef }` — with **one codec**
(`workspace/model/pane-item.ts`) between it and split panes' key: `s:` then the
session id; `w:` then the plugin and the id, each percent-encoded, joined by
`:`. The encoding is one-to-one and canonical over any string through the
desktop's one id encoder (`src/desktop/model/id-encoding.ts`, which the
subagents join and the swarm's ids use too: it encodes UTF-16 code units, so a
lone surrogate encodes too, where `encodeURIComponent` would throw), and two
items are the same pane exactly when they are equal; a property test holds both.
The key is a branded type only the codec makes; reading one back goes through
the codec's `decode`, which is the only thing the module exports that takes a
key. Split panes take any string, so the brand cannot keep a hand-built one
out of their calls; the workspace hands them only what the codec writes, and
its tests read every pane through `decode`, refusing a key it did not write. The panes' use cases take the union;
what is showable, on screen, or kept as a draft is decided per kind.

A transcript part `{ kind: "widget", widget: WidgetRef }` is drawn by
`InlineWidget`. The overview's peek (`peek.ts`) drops it before it counts a
message's parts; step grouping treats it as a break, as it does code; the
readers that take a message's text (`messageText`, the drag's card) already pass
over a kind they do not know. What produces widget parts over the wire is the
gateway's, and remaining.

### What a host draws

| `useWidget` answers | Plugin registered | `inline` | `pane`, `window` |
| --- | --- | --- | --- |
| ready | yes | the plugin's inline view, or a row with its title and Open if it has none | the plugin's view for that place; a place it does not offer is not offered |
| unread | yes | a quiet placeholder with its plugin's name | the same, in the place's chrome |
| missing | yes | "This is no longer available" | the same line, with close |
| off | yes | "Turned off in Settings › Advanced › Experimental" | the same line, with close |
| unshowable | yes | "Can't show this here" | the same line, with close |
| — | no | "Can't show this here" | the same line, with close |

An inline widget keeps its mounted view across surrounding message-part changes,
keyed by widget identity and its occurrence among that identity (#616,
`message-widget.test.tsx`). Repeated references remain distinct siblings. Removing
or inserting indistinguishable occurrences does not promise per-occurrence state
retention; that would require an occurrence identity in the source contract.

Nothing is retried by the host and nothing pretends to be live (gate 7, gate
16); a source that reads again answers `useWidget` again.

An `app` plugin's answer is its tool call's (#349): `unread` while the call is
not read, `missing` for none, else ready, titled by its tool and belonging to
the call's session; it offers every place. A ready app's view is the sandboxed
frame, and has rows of its own (`app/model/app-view.ts`):

| The app's view | `inline` | `pane`, `window` |
| --- | --- | --- |
| loading (its resource read, the proxy loading, the handshake) | its plugin's name, quiet, over the frame not yet shown | the same |
| live | the frame | the frame |
| could not be loaded (not an app's resource, a deadline missed, its proxy reloaded under it, the app's document gone from its frame) | "This app couldn't be loaded" | the same, with close |
| its server gone when read | "This app's server has stopped" | the same, with close |
| live, a call found its server gone | the frame, under "This app's server has stopped" | the same |
| from loading on, its CSP blocked a load | what it draws, under "Blocked a connection this app didn't declare: *origins*" | the same |

### Boundaries

The desktop's verticals depend in one direction: **widgets ← workspace ←
subagents ← experiments**, and subagents and experiments on widgets' contract.
Widgets imports none of them; the workspace imports widgets' hosts and no
plugin; subagents imports no experiment.
`scripts/architecture/desktop-verticals.mjs` refuses an import against that
direction, in the forms of import `split-panes-boundary.mjs` reads.

## Alternatives considered

- **Widgets as a kind of session.** A widget pane as a session with no
  transcript. It lost because a session carries drafts, retention, approvals
  and the overview, and every one of those rules would have to learn to skip
  it.
- **A separate layer over the grid.** Floating panels outside split panes. It
  lost because panes already solve placement, resizing, dragging and focus,
  and a second system for the same room would disagree with the first.
- **Keep the string key and parse it where needed.** The prototype's shape,
  and the reason every reader had to know about widgets and two items could
  collide.
- **A column attached inside a conversation's pane.** The prototype had one.
  It needed its own rules for which session it belongs to, for fitting when a
  split or resize narrows the pane, and for following its session between
  panes — state a pane beside the conversation already has. Dropped after two
  review rounds kept finding cases in it.
- **Plugins registering themselves on import.** Load order becomes behaviour,
  and tests register whatever happened to be imported.

## Consequences

- A plugin is a vertical exporting one plugin object, registered in one line of
  composition; the host never changes for it.
- Every workspace rule that reads a pane's item narrows on its kind — a wide,
  mechanical change in #327, paid once.
- Split panes are unchanged: they still see an opaque item.
- Remaining: what puts widget parts into a message over the wire, which is the
  gateway's.
- Watch for plugins that want state shared between their places (a tab, a scroll
  position): what a plugin shares between its places is the plugin's to keep,
  never the host's — the subagents panel keeps which subagent it shows per
  conversation, so the same panel in a pane and in the window shows the same
  one; an experiment keeps its trail per view.
- Work: #327 (the pane-item union and codec, the panes' and the content
  view's use cases) and #328 (the registry, the hosts, the widget pane's
  chrome, the transcript part, Escape and focus, the boundary rule, the
  module map in `docs/codebase-structure.md`, `widgets.mjs`). Part of #325.

## Evidence (#328)

- **Registration and the table**, as plain functions: `application/registry.test.ts`
  (a duplicate id refused at composition with `WidgetRegistryError`, and at
  run time against a native and an `app` plugin, changing nothing; a native
  plugin never unregistered), `model/host-table.test.ts` (every row of _What a
  host draws_, in every place), `application/escape-stack.test.ts`.
- **The hosts**, in jsdom: `ui/hosts.test.tsx` (each row drawn by the card and
  by the body; a plugin registered and unregistered while a host is on the
  page; a different plugin under one id read by a fresh reader).
- **The workspace**, in jsdom, through the whole shell and the sample plugin:
  `workspace/ui/layouts/widgets.test.tsx` (the card's pane beside its
  conversation, the trail back, close, ⌘1–4 and ⌘W, the accessory, the window
  left every way, Escape's order, focus on open and close),
  `widget-escape.test.tsx`, `use-edge-peek.test.tsx`.
- **The boundary**: `scripts/architecture/desktop-verticals.mjs`, refusing
  every later vertical in each form of import (`desktop-verticals.test.mjs`).
- **In real browsers**: `verification/desktop/scripts/widgets.mjs`, twelve checks
  in Chromium and WebKit, both layouts (`verification/desktop/CHECKLIST.md` ›
  _Widgets_). Building it found the edge peek's Escape attached an effect after
  the reveal was on the page: Escape pressed in between dismissed nothing
  (2 of 8 runs in WebKit). The peek now listens for its life, in
  capture, and marks Escape handled; 40 of 40 held after.
