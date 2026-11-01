# Review brief: `read_window` reports the focused chat's panel

Give this to a local review of issue #1120, with
[CODING_STANDARDS.md](../../CODING_STANDARDS.md) (the required review brief,
the agreement check, and gates 10, 13 and 15).

## Checkout

- Repository: `/workspace` (`nessalabs/nessa-agent`)
- Branch: `cursor/read-window-panel-1a19`
- Base: `ca8c33bf54b920b24639a36847078ef40359c840` (`origin/main` when the branch was cut)
- Head: the pull request head. Re-read `git rev-parse HEAD` and `git status`.
  A dirty tree is in scope; a head SHA does not name it.

## Intended behavior

`workspace.publish` includes the focused chat's right panel, and `read_window`
says it. An agent can tell that a widget (the subagents widget, an MCP App)
is open in the panel.

The decision, the state table, the invariants and the alternatives are
[ADR 1120](../adr/todo/1120-read-window-panel.md). Review the implementation
against that table. A new state or ordering goes into the table before it
goes into code.

## Owned scope

- Product schema `WorkspaceWindow.panel` and generated Rust and TypeScript.
  No second shape, no reader of the old window.
- Domain `WindowPanel` as the wire's owner of front-is-a-tab, no duplicates,
  and the 16-tab bound.
- Desktop `shownWindow` and the layout's `panelShown`. The chat panel model
  stays the owner of opening, showing and closing a tab.
- `read_window` words, and the `nessa-agents` tool description. Claude and
  Codex only; do not add an OpenCode surface.
- Tests of the five table rows at the publisher and at `read_window`.

## Out of scope

- Widget pane items and a widget drawn over the panes (`showing`). Those stay.
- Maximized as its own fact. An expanded panel reads as open with its front tab.
- Publishing every chat's panel.
- A conversation-stress step. The existing `window` step already calls
  `read_window`. Say so if that is no longer true.
- Cedar. Who may publish and who may read does not change.

## What to falsify

- A front that is not a tab, a repeated tab, 17 tabs, a front with no tabs,
  and a window with no panel are each `invalid_request` and leave the held
  window unchanged.
- Closed still lists the focused chat's tabs. Open with no tabs lists none
  and names no front.
- Switching chats changes tabs and front and does not change `open`.
  The other chat's tabs are absent.
- Bringing a tab to the front changes `front` and not the tab order.
- The last tab closing leaves `open` as it was, with no tabs and no front.
- The Agents overview, or a widget over the panes, does not drop the panel.
- The layout is the only writer of `open`. `shownWindow` does not read the
  preference. A rail item that covers the workspace publishes closed.
- Two implementations of "which tab is in front" or of the tab bound. The
  desktop constructors and `WindowPanel::new` are the two layers named in
  the decision; a third check is a finding.

## Dimensions that apply

Authority and identity, domain invariants, representation boundaries, resource
bounds, and public surface. Admission, cleanup, audit and restoration do not
change: a window is still held for the connection and withdrawn when it
closes or the publication is refused for good. Say that exclusion in the
report, and which of the relationships above were traced (focused chat, drawn
panel, tabs, front) from the layout through `shownWindow`, the wire, the
domain and `read_window`.

## Agreement

The focused session, the panel drawn, the tabs and the front are four facts.
The focused session comes from the pane layout. The drawn panel comes from
the layout's `panelShown`. The tabs and front come from that session's
`ChatPanel`. `WindowPanel::new` is what a wire must satisfy before it replaces
the held window. A review that only lists the new types fails this check.
