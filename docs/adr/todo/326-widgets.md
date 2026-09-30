# 326. A plugin's view is a widget the window hosts, in a message, beside it, or in a pane

## Purpose

Some of what a conversation produces is better seen than read: an experiment
a swarm is running ([333](333-experiments.md)), the subagents a conversation
has put to work ([329](329-subagents.md)), and later whatever else a plugin
draws. This record settles how the desktop window hosts such a view — inline
in a message, in a column beside the conversation, or in a pane of its own in
the split grid ([253](../done/253-split-panes-component.md)) — without the
window knowing any plugin, and without any plugin knowing the workspace.

- **Date:** 2026-09-30
- **Status:** proposed

## Context

A prototype (`exp-prototype` @ `5bfaa225`, `src/desktop/widgets/`) showed the
three places and the way between them: a card in the assistant's message, a
button that opens it beside the conversation, an expand that gives it a pane,
and Escape back. People liked the flow. Its structure is what cannot ship:

- **The host imports its plugins.** `widgets/ui/registry.tsx` imports the
  experiments and subagents views to build a static table, so the generic
  host depends on every plugin, and a new plugin edits the host.
- **A widget pane borrows the session-id slot.** A pane showing a widget holds
  `widget:<plugin>:<id>` where a session id goes (`workspace/model/pane-item.ts`),
  so every reader of a pane's item — what is showable, what is on screen, what
  drafts to keep, the header, the drag's card — has to know the string might
  not be a session, and each learns it separately (gate 13).
- **Nothing says what happens when a widget cannot be drawn.** An unknown
  plugin, or a widget its plugin no longer has, rendered nothing.

What binds:

- **Split panes stay host-agnostic.** A pane shows an opaque `item` string
  (253); the meaning of the string is the host's, and stays so.
- **A plugin owns its data.** An experiment is read from `ExperimentSource`,
  subagents from `SubagentSource`; the widget layer must not become a second
  path to either.
- **The transcript is a projection of the source.** A widget reaches a message
  as one of its parts, like text and steps, not by the view reaching into the
  window.

## Decision

`src/desktop/widgets/` owns the widget contract and its hosts, and knows no
plugin. A **widget** is `{ plugin, id }`: the plugin that draws it and the
plugin's own id for the thing drawn. A **plugin** is `{ id, title(id),
Inline?, Beside?, Pane }` — views given only the widget's id and the host's
callbacks (`open(place)`, `close()`), which read their data from their own
vertical's ports. Plugins are registered once, in composition (`main.tsx`),
into a `WidgetRegistry` provided to the tree; the host looks a plugin up by id
through the registry and nothing else.

In the workspace, a pane's item is a **tagged union** — `{ kind: "session",
sessionId } | { kind: "widget", widget }` — with one codec (`model/pane-item.ts`)
that turns it into the opaque key split panes store and back. Every reader
decodes through it; none parses a key. The panes' use cases take the union:
`openSession` stays for sessions, `openWidget` and `openWidgetBeside` for
widgets, and what is showable, on screen, or kept is decided per kind.

A transcript part `{ kind: "widget", plugin, id }` is drawn by `InlineWidget`.
The **beside** column is a pane's own (one per pane, held by the pane, closed
by Escape or its close); a **pane** of its own is an ordinary split pane whose
header is the widget pane's: the plugin's title, a breadcrumb back to the
conversation it came from when there is one, close and expand.

What a host draws, by what it finds:

| The registry has the plugin | The plugin has the widget | Inline | Beside / pane |
| --- | --- | --- | --- |
| yes | yes | the plugin's `Inline`, or a plain "Open" row if it has none | the plugin's view |
| yes | no | "This is no longer available" | the same line, with close |
| no | — | "Can't show this here" | the same line, with close |

Nothing is retried and nothing pretends to load (gate 7, gate 16). A widget
pane lives as long as the layout does, exactly as a session pane does; the
layout is not persisted today, and this record does not change that.

## Alternatives considered

- **Widgets as a kind of session.** A widget pane would be a session with no
  transcript. It lost because a session carries a lifecycle — drafts,
  retention, approvals, the overview — that a widget has none of, and every
  one of those rules would have to learn to skip it.
- **A separate widget layer over the grid.** Floating panels outside split
  panes. It lost because panes already solve placement, resizing, dragging and
  focus, and a second system for the same room would disagree with the first.
- **Keep the string key and parse it where needed.** Cheapest now; it is the
  prototype's shape and the reason every reader had to know about widgets.
- **Plugins discovered by import side effect.** Each plugin registering itself
  when its module loads. It lost because load order becomes behaviour, and
  tests would register whatever happened to be imported.

## Consequences

- A plugin is a vertical that exports a plugin object and is registered in one
  line of composition; the host never changes for it.
- Every workspace rule that reads a pane's item now narrows on its kind. That
  is a wide, mechanical change in #327, paid once.
- The split-panes module is unchanged: it still sees an opaque item.
- A widget cannot outlive its plugin's registration silently: the table above
  says what is drawn, and the browser checks hold it.
- Watch for plugins that want state shared between their inline card and their
  pane (a selected tab, a scroll position). Today each view holds its own; if
  that stops being right, the registry, not the host, is where a plugin would
  keep it.

## Work

| Issue | Scope |
| --- | --- |
| #327 | The pane-item union, its codec, and the panes' use cases on it |
| #328 | The registry, the three hosts, the widget pane, the transcript's widget part, and `widgets.mjs` |

Part of #325.
