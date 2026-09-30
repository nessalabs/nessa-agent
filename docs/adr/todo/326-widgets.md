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
- **Status:** proposed

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
- **The content region already has a second view.** The Agents overview
  (⌘0) is drawn instead of the panes, which stay as they were beneath it; 238
  says how it is left: Escape, or going anywhere else — a session chosen, any
  change of the panes — is going back to them (`ContentView`,
  `usecases/navigation.ts`, `goesSomewhere` and `changesPanes` in the slice).

## Decision

### The contract

`src/desktop/widgets/` owns the contract and the hosts, and knows no plugin.

- A **`WidgetRef`** is `{ plugin, id }`: the plugin that draws it and the
  plugin's own id for the thing drawn. It is the one shape everywhere — in a
  pane item, in a transcript part, in a command.
- A **plugin** is `{ id, name, useWidget(id), views, SessionAccessory? }` —
  `name` is what the window calls it while a widget is not read yet
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
  `onEscape(handler)` (below).
- Plugins are **registered once, in composition** (`main.tsx`), into a
  `WidgetRegistry` provided to the tree. Two plugins with one id is a
  composition error, raised when the registry is built and tested there. The
  host looks a plugin up by id through the registry and nothing else.

### Places

| Place | What it is | Opened by | Left by |
| --- | --- | --- | --- |
| `inline` | a card in the message the widget part is in | the transcript | — |
| `pane` | a pane of its own in the split grid, as a chat has | `open("pane")`: beside the pane showing its `origin`, by the workspace's `openBeside` and its rules, or in the focused pane's place when there is none | the pane's close or ⌘W, as any pane; the last pane, as 238 has it, goes back to a new session's home, where a new session goes (`createDraft`: the channel being looked at, else the first) — `closePane` gains that case, which today leaves a last pane not showing a session as it is |
| `window` | the content region's third view, beside the panes and the overview: the widget drawn instead of the panes, which stay beneath it as they were | `open("window")` | as the overview is — Escape or its close, a session chosen, or any change of the panes goes back to the panes; ⌘0 goes to the overview — with one difference: ⌘W closes the window, never a pane beneath it (over the overview, ⌘W closes the focused pane, 238) |

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
focus in the widget's pane, where the next Escape still reaches it. Opening a
widget in a pane is a pane taking focus; opening one in the window moves focus
into it; going back to the panes is the focused pane taking focus.

**Escape**, after 238's owners — a menu or dialog, a carrying drag, the edge
peek, the open overview — goes to the widget in front. While the content view is
a widget, the window covers the content region only, and the session list beside
it stays in reach; Escape anywhere outside a menu, dialog, carrying drag or
shown edge peek (each of which keeps its Escape and goes no further) — the
session list's rows included — goes to the window's widget. In the session
list's search, an Escape with a query clears it and goes no further; the next,
with none, goes to the window's widget. Otherwise it is a pane's widget, only
while focus is inside that pane, so a field elsewhere keeps its Escape. The view
has the first refusal (a view with somewhere to step back to — a run it opened,
a subagent it shows — registers `onEscape` and handles it); then the host, which
goes back to the panes from the window. A widget in a pane is not closed by
Escape, as a session pane is not.

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
The key is a branded type only the codec makes, so no reader can build one by
hand; reading one back goes through the codec's `decode`, which is the only
thing the module exports that takes a key. The panes' use cases take the union;
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

Nothing is retried by the host and nothing pretends to be live (gate 7, gate
16); a source that reads again answers `useWidget` again.

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
