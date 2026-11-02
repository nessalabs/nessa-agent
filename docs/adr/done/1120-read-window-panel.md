# 1120. `read_window` says the focused chat's right panel

## Purpose

An agent reading the desktop with `read_window` should be able to tell that a
widget is open in the focused chat's right panel. Since widgets became tabs
there ([326](326-widgets.md), amended), the publication still described
only panes and a widget drawn over the window.

- **Date:** 2026-11-01
- **Status:** accepted

## Context

`workspace.publish` is the window's account of what it shows, and `read_window`
is the only way an agent hears it ([the window an agent shares](../../design/agent-window.md)).
The panel is a fact of that window: one panel is drawn, it belongs to the
focused chat, and it has tabs with one in front (`model/chat-panel.ts`).
Whether it is drawn is the layout's, remembered as a preference and closed
again while a rail item covers the workspace (`use-workspace-window.ts`). The
tabs are the chat's, in the workspace store.

A publication that omits the panel says the window is panes and nothing else.
That is what an agent hears today when the subagents widget or an MCP App is
the tab in front.

## Decision

`WorkspaceWindow` carries the focused chat's panel: whether it is drawn
(`open`), its tabs in the order they were opened (plugin and id), and the tab
in front. `read_window` says the same facts in words. A publication without a
panel is not a window. Other chats' panels are not included; focusing that
chat is what publishes them. Claude and Codex both call this one
`nessa-agents` tool. No OpenCode surface is added.

## States

The published panel is one of these. `open` is the window's, not the chat's.
`tabs` and `front` are the focused chat's. Each row has a test at the desktop
reading (`shownWindow`), the publisher's wire, and `read_window`'s words.

| State | `open` | `tabs` | `front` | What moves it |
| --- | --- | --- | --- | --- |
| Panel closed | false | the focused chat's tabs, or none | that chat's front tab, or none when there are no tabs | The layout says the panel is not drawn |
| Open with tabs | true | those tabs, in the order they were opened | the tab the chat's panel names | The layout says it is drawn, and the chat has tabs |
| Front tab changes | unchanged | unchanged | the tab brought to the front | `showInPanel` |
| Chat switch | unchanged | the newly focused chat's tabs | that chat's front tab | The focused pane's session changes |
| Last tab closed | unchanged | none | none | `closeInPanel` drops the chat's panel; the chrome stays as the layout left it |

A chat with no panel entry publishes no tabs and no front. A focused pane that
is not a session (a draft is a session) publishes no tabs. Closing the panel
does not drop the tabs. Opening it does not invent any.

## Invariants

Each guarantee names the code that enforces it.

| Guarantee | Enforcer |
| --- | --- |
| The published panel is the focused chat's only | `shownWindow` reads `sessionIn` of the focused pane and that session's `panels` entry |
| `open` is whether the panel is drawn, including while a rail item covers the workspace, and is not recomputed from the preference | The layout (`panelPlace.open` and no covering rail item) records it with `panelShown`; `shownWindow` reads that field |
| `front` is absent exactly when `tabs` is empty, and is otherwise one of `tabs` | `WindowPanel::new` on the wire. On the desktop, `openInPanel`, `showInPanel` and `closeInPanel` are the only constructors of a chat panel |
| No tab is repeated | `WindowPanel::new`. On the desktop, `openInPanel` brings an existing tab to the front or replaces its plugin's one tab |
| At most 16 tabs, each plugin and id within the existing widget id bound | `WindowPanel::new` (`MAX_PANEL_TABS`) and `WidgetRef::new`. The product schema's `WorkspacePanel.tabs` `maxItems` is that same 16 |
| A window with no panel, a front that is not a tab, or a front beside no tabs is refused `invalid_request` and changes nothing held | `workspace.publish` decodes into `WindowPanel::new` and keeps the previous window on refusal |
| `showing` (panes, the Agents overview, a widget over the panes) and the panel are independent | `shownWindow` copies each from its own field |

`read_window` says `open` as `"open"` or `"closed"`, the same words it uses for
the sidebar and the session list. The wire field stays a boolean, as those do.

## Alternatives considered

- **Publish a panel widget as a pane item.** That is the old path. A widget is
  not a pane ([326](326-widgets.md)); saying it is one tells the agent
  the window has a pane it does not draw.
- **Omit `panel` when it is closed, or omit `tabs` when it is closed.** Closed
  with tabs, open with none, and a publisher that forgot the field become the
  same absence. The closed row has to be sayable, and a missing field has to
  be a refusal.
- **Publish every chat's panel.** The window draws one panel. The others are
  not on screen. An agent learns them when that chat is focused, which is
  already how the window works. A list of every chat's tabs grows the tool
  result and describes something the person is not looking at.
- **Have `shownWindow` read the panel preference itself.** The preference is
  the remembered choice. The panel is also undrawn while a rail item covers
  the workspace. The layout already owns that combination, the same way it
  owns which arrangement is on screen (`arrangementShown`). A second reading
  of the preference would disagree with the layout.
- **Accept a publication that still has no panel, and treat it as closed.**
  That is a second owner of what a window is, kept for a shape this build no
  longer sends. Nessa is alpha: the old shape is refused.

## Consequences

Agents can tell a widget is the tab in front, and whether that panel is drawn.
A maximized panel is still `open` with that front tab; `covered` stays the
overview or the widget over the panes, and this decision does not add a third
covered cause. The 16-tab bound is past a readable strip. A chat that somehow
held more would be refused as a window and the publication withdrawn, the same
as too many columns.

The stress `window` step asks `read_window` through the live desktop. It
opens two panel widgets on the focused chat, brings the other tab to the
front, hides and shows the panel, and focuses the other chat, and each of
those is what `read_window` says.
