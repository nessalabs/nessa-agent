# 326. A plugin's view is a widget the window hosts, in a message, beside it, in a pane or over the panes

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
flow people liked: a card in the assistant's message, a button that opens it in
a column attached to the conversation, an expand that gives it a pane, a
full-window view with a breadcrumb, and Escape back. Its structure is what
cannot ship:

- **The host imports its plugins.** `widgets/ui/registry.tsx` imports the
  experiments and subagents views to build a static table, so the generic host
  depends on every plugin, and a new plugin edits the host.
- **A widget pane borrows the session-id slot.** A pane showing a widget holds
  `widget:<plugin>:<id>` where a session id goes (`workspace/model/pane-item.ts`).
  Every reader of a pane's item — what is showable, on screen, kept as a draft,
  the header, the drag's card — has to know the string might not be a session;
  and a session id that began `widget:`, or a widget id with a `:` in it, would
  collide with another item, which split panes treat as the same pane
  (`split-panes/model/pane-layout.ts`, `paneShowing`).
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

## Decision

### The contract

`src/desktop/widgets/` owns the contract and the hosts, and knows no plugin.

- A **`WidgetRef`** is `{ plugin, id }`: the plugin that draws it and the
  plugin's own id for the thing drawn. It is the one shape everywhere — in a
  pane item, in a transcript part, in a command.
- A **plugin** is `{ id, useWidget(id), views, SessionAccessory? }`.
  `useWidget` is the plugin's hook answering, reactively, `{ kind: "ready",
  title, origin? } | { kind: "unread" } | { kind: "missing" }` — `origin` is
  the session the widget belongs to, when it has one. `views` offers any of
  `inline`, `attached`, `pane`, `window` (`pane` at least). A
  `SessionAccessory` is something the plugin draws in a session pane's header
  for that session (the subagents' avatar stack), so the workspace draws it
  without importing the plugin.
- Views are given only the widget's id and the **host's callbacks**:
  `open(place)`, `close()`, `openWidget(ref, place)` (a plugin opening another
  plugin's widget — an experiment's agent opening the subagents panel), and
  `onEscape(handler)` (below).
- Plugins are **registered once, in composition** (`main.tsx`), into a
  `WidgetRegistry` provided to the tree. Two plugins with one id is a
  composition error, raised when the registry is built and tested there. The
  host looks a plugin up by id through the registry and nothing else.

### Places

| Place | What it is | Opened by | Closed by |
| --- | --- | --- | --- |
| `inline` | a card in the message the widget part is in | the transcript | — |
| `attached` | a column inside a session's pane, beside its conversation | `open("attached")` | its close, or Escape |
| `pane` | a pane of its own in the split grid | `open("pane")`, a drop | the pane's close (as any pane) |
| `window` | the widget fills the content region over the panes, as the Agents overview does | `open("window")` | its close, or Escape: the panes as they were |

A widget is in **at most one** of `attached`, `pane` and `window` at once;
opening it in another moves it there (its inline card stays in its message).
The attached column is held in the workspace's state **keyed by the session**
it is attached to, not by the pane: a pane that comes to show another session
shows that session's column, if any, and the first session's comes back with
it. It is offered only where the pane fits it — the fit rule stays 238's
window fit, extended with the column's minimum — and where it does not fit,
Open goes to a pane. A session that is removed takes its attached widget with
it; a widget in a pane or the window whose `origin` is removed stays, without
a way back to a conversation.

**Focus** follows 238: opening a widget in a pane focuses that pane, as
opening a session does, and the pane's body takes focus for the view to place
further; opening it attached or in the window moves focus into it; closing
returns focus to the conversation it came from, or to the focused pane.

**Escape**, outside a menu or dialog and when no drag is carrying: the view
first — a view that has somewhere to step back to (a run it opened, a
subagent it is showing) registers `onEscape` and handles it — then the host,
which closes `attached` or leaves `window`. A widget in a pane is not closed
by Escape, as a session pane is not. The window's content view is the panes,
the overview, or a widget; the overview's own Escape is unchanged.

The window's **layout is not persisted** (238), and widgets do not change that:
a widget pane, an attached column and the window view live as long as the
window does.

### In the workspace

A pane's item is a **tagged union** — `{ kind: "session", sessionId } | { kind:
"widget", widget: WidgetRef }` — with **one codec** (`workspace/model/pane-item.ts`)
between it and split panes' key: `s:` then the session id; `w:` then the
plugin and the id, each percent-encoded, joined by `:`. The encoding is
one-to-one and canonical, so two items are the same pane exactly when they are
equal (a property test holds it). Every reader decodes through the codec; none
parses a key. The panes' use cases take the union; what is showable, on
screen, or kept as a draft is decided per kind.

A transcript part `{ kind: "widget", widget: WidgetRef }` is drawn by
`InlineWidget`. Text search and the overview's peek skip it (it has no text of
its own); step grouping treats it as a break, as it does code. What produces
widget parts over the wire is the gateway's, and remaining.

### What a host draws

| `useWidget` answers | Plugin registered | `inline` | `attached`, `pane`, `window` |
| --- | --- | --- | --- |
| ready | yes | the plugin's inline view, or a row with its title and Open if it has none | the plugin's view for that place; a place it does not offer is not offered |
| unread | yes | a quiet placeholder with its plugin's name | the same, in the place's chrome |
| missing | yes | "This is no longer available" | the same line, with close |
| — | no | "Can't show this here" | the same line, with close |

Nothing is retried by the host and nothing pretends to be live (gate 7, gate
16); a source that reads again answers `useWidget` again.

### Boundaries

The desktop's verticals depend in one direction: **widgets ← workspace ←
subagents ← experiments**, and subagents and experiments on widgets' contract.
Widgets imports none of them; the workspace imports widgets' hosts and no
plugin; subagents imports no experiment. `scripts/architecture/desktop-verticals.mjs`
refuses an import against that direction, in the forms of import
`split-panes-boundary.mjs` reads.

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
- **The attached column keyed by pane.** Simpler state, but a pane that shows
  another session would show the first session's widget beside it.
- **Plugins registering themselves on import.** Load order becomes behaviour,
  and tests register whatever happened to be imported.

## Consequences

- A plugin is a vertical exporting one plugin object, registered in one line of
  composition; the host never changes for it.
- Every workspace rule that reads a pane's item narrows on its kind — a wide,
  mechanical change in #327, paid once.
- Split panes are unchanged: they still see an opaque item.
- Watch for plugins that want state shared between their places (a tab, a
  scroll position): today each view holds its own; if that stops being right,
  the plugin keeps it, not the host.
- Work: #327 (the pane-item union and codec, the panes' and content view's use
  cases, the attached column's state, the fit rule) and #328 (the registry,
  the hosts, the widget pane's chrome, the transcript part, Escape and focus,
  the boundary rule, `widgets.mjs`). Part of #325.
