# Graduating the agents overview

The plan to take the overview from an experiment to a view of the workspace.
It lists the edits for each shared file, to apply once the workspace's
current edits have landed. Nothing here has been applied yet. This file goes
away in the change that applies it.

## Recommendation: move it into the workspace vertical

Its state belongs in the workspace slice (ADR 0001: agents drive it), and it
changes the workspace's answering rule and retention. So it becomes part of
the workspace rather than a sibling that reaches into the slice's internals.
The workspace's own read is now `WorkspaceIndex`, so "overview" no longer
collides with anything and is the name used below.

| From `experiments/agents-overview/` | To `workspace/` |
| --- | --- |
| `model/agents-glance.ts`, `filter.ts`, `walk.ts`, `peek.ts`, `request.ts` (and their tests) | `model/overview/` (same files) |
| `adapters/reflow.ts`, `surface-width.ts` | `adapters/dom/overview-reflow.ts`, `adapters/dom/width.ts` |
| `adapters/workspace-bridge.ts` | deleted: its selectors move to `adapters/store/selectors.ts`, and its two thunks are replaced by the workspace's own commands (below) |
| `adapters/preference.ts` (filter) | deleted: the filter becomes slice state (below) |
| `ui/*` | `ui/overview/` (`overview.tsx`, `request-row.tsx`, `session-row.tsx`, `session-peek.tsx`, `reply-pill.tsx`, `filter-menu.tsx`, `all-clear.tsx`, `overview-keys.ts`, `overview.css`, `overview.test.tsx`) |
| `ui/overview-scope.tsx` | split: the entry becomes `ui/source-list/overview-row.tsx`; the area becomes `ui/panes/overview-area.tsx`; the scope and its context go away (the slice holds `open`, and the shell reads `selectOverviewOpen` for `data-content`) |

## State: named actions per ADR 0001

`application/workspace-state.ts`:

```ts
export interface OverviewState {
  readonly open: boolean
  /** The session the peek shows; kept even when a filter hides it. */
  readonly selected: string | null
  readonly filter: AgentsFilter // from model/overview/filter.ts
}
// WorkspaceState gains: readonly overview: OverviewState
// initialWorkspace: overview: { open: false, selected: null, filter: defaultFilter }
```

`application/usecases/overview.ts` (new, pure) and `adapters/store/slice.ts`
add these actions, re-exported from `commands.ts`:

- `showOverview({ open?: boolean })`: toggles when `open` is absent.
- `selectInOverview({ sessionId })`: an id not listed is not selected.
- `filterOverview({ filter })`.
- Rule (use case): a change of the focused pane's session (`openSession`,
  `newSession`, …) sets `overview.open = false`. It is written once, in the
  reducer, not in a component.

The filter stays remembered, as it is now. The slice owns it, and a listener
effect in `effects.ts` writes each change to the stored preference key the
experiment uses today. `dependencies.ts` reads that key once, at
composition, into the initial state. Storage is persistence only, never a
second owner.

## Answering and loading: replacing the bridge

**The rule (gate 13):** the overview shows a session when it is open and the
session is either waiting on the person or the selected one. Written once:

```ts
// application/workspace-state.ts
export function overviewShows(state: WorkspaceState, sessionId: string): boolean {
  const { open, selected } = state.overview
  return open && (selected === sessionId || sessionOf(state, sessionId)?.status === "needs-you")
}
/** On screen: in a pane, or in the open overview. */
export function onScreen(state: WorkspaceState, sessionId: string): boolean {
  return (state.panes !== null && paneShowing(state.panes, sessionId) !== undefined) ||
    overviewShows(state, sessionId)
}
```

- **`adapters/store/commands.ts`, `answer()`:** replace the `shown`
  expression with `onScreen(state, sessionId)`.
- **`application/usecases/updates.ts`:** replace
  `shownSessionIds(state)` with `shownSessionIds(state)` ∪ the sessions
  `overviewShows`. Use it for both reading and retention.
- **`model/retention.ts`:** keep those conversations too, bounded:
  `retention.overviewConversations = 24`, the most recently active first.
  Past the bound, a session's conversation is read when its row is selected.
- **`adapters/store/effects.ts`:** the first listener already reads every
  "shown" conversation not yet held. With the union above it reads the
  overview's too; nothing else changes. Marking read stays with panes only:
  a peek is not reading the session.
- **Overview UI:** dispatches `approve` / `deny` (initiator `"person"`) and
  reads `selectTranscript`. `readConversation` and `answerRequest` are
  deleted along with the bridge.

**Gate 15 rows for ADR 238's "Commands in flight" table** (each gets a test
in `commands.test.ts` / `updates.test.ts`):

| Command | Shown at once | Source takes it | Source refuses (typed) | Source's update arrives |
| --- | --- | --- | --- | --- |
| `approve` / `deny` for a session the open overview shows (waiting, or selected), in no pane | the row's answers at rest; the row settles in place | as for a pane: the conversation that no longer asks, delivered first, lets the answer go | asks again, saying why, in the row and its peek | the session moves to Working; the row leaves once its settle has played |
| `approve` / `deny` for a session neither a pane nor the open overview shows | nothing; `not-asked` | — | — | — |
| the overview closes while an answer is on its way | nothing | the answer is kept; as for a pane that shows another session | its reason is kept for when it is shown again | as above |
| `sendMessage` from the peek's reply pill while an approval waits | the message in the outbox, "sending"; the pill says replying sets the request aside | the approval is let go, on the source's record with who sent it (unchanged) | "Not sent" under the pill; Send Again in the pane | as for a pane |

## Exports for `workspace/index.ts`

These are only needed if the overview stays a sibling. With the move above,
most become internal imports. Still export these for Settings and composition:

```ts
export { showOverview, selectInOverview, filterOverview } from "./adapters/store/commands" // via export *
export { selectOverviewOpen } from "./adapters/store/selectors"
```

Keep as sibling instead? Then export: `agentOf`, `agentName`, `modelName`,
`sessionTime`, `useNow`, `labelOf`, `matchesChord`, `Binding`, `failureCopy`,
`AgentTile`, `StatusGlyph`, `LiveRow`, `ToolSteps`, the types
`SessionSummary`, `Transcript`, `Approval`, `WorkspaceFailureReason`, and
selectors `selectSession`, `selectTranscript`, `selectChannel`,
`selectComposerText`, `selectOutbox`, `selectFocusedSessionId`, and a
glance selector.

## What is already in shared files (Phase A, applied with the lead's go-ahead)

- `workspace/ui/layouts/three-columns.tsx`, `sessions-in-sidebar.tsx`: the
  shell is wrapped in `<AgentsOverviewScope>`.
- `workspace/ui/layouts/workspace-shell.tsx`: the pane grid is wrapped in
  `<AgentsOverviewArea>`. `useAgentsOverviewShown()` sets
  `data-content="agents" | "panes"` on the root and leaves out the session
  list's resize edge. `data-panes-alone` also counts the list as away while
  the overview shows. The overview's CSS hides `.workspace-list` under
  `data-content="agents"`, so the list stays mounted.
- `workspace/ui/source-list/source-list.tsx`: with the experiment on,
  `StatusViews` renders only `<AgentsOverviewEntry />` (with the waiting
  count) in place of "Needs you" and "Running". `TreeTop` puts the entry
  after Search and drops its "Needs you" button.
- `workspace/adapters/dom/shortcuts.ts`: `Backspace: "⌫"`.
- `settings/…`: the Experimental tab and its toggle.

## Exact edits per shared file (after the move)

1. **`workspace/ui/layouts/three-columns.tsx`, `sessions-in-sidebar.tsx`:**
   remove the `<AgentsOverviewScope>` wrapper and its import.
2. **`workspace/ui/layouts/workspace-shell.tsx`:**
   - `useAgentsOverviewShown()` becomes `useWorkspaceSelector(selectOverviewOpen)`,
     in both places.
   - `<AgentsOverviewArea>` becomes `<OverviewArea>` (`../panes/overview-area`).
   - Add a `"showOverview"` case to `useWorkspaceKeys` dispatching
     `showOverview()`, bound in `ui/layouts/shortcuts.ts` as
     `bind({ code: "Digit0", command: true }, "showOverview")`. Add
     `"showOverview"` to `ShortcutCommand` in `ui/workspace-frame.tsx`. The
     scope's own ⌘0 listener goes.
   - Move the `.workspace[data-content="agents"] .workspace-list` rule into
     `ui/layouts/layouts.css`.
3. **`workspace/ui/source-list/source-list.tsx`:**
   `useAgentsOverviewEnabled()` becomes the Settings decision below. If the
   view is always there, `StatusViews` and `TreeTop` lose their "Needs you" /
   "Running" rows for good and render `<OverviewRow />`
   (`./overview-row`, reading `selectOverviewOpen` and the waiting count,
   dispatching `showOverview()`).
4. **`settings/ui/settings-tabs.tsx`, `settings/model/settings-catalogue.ts`:**
   depends on the decision below. If the toggle goes, remove the
   `experimental` tab, the `agents-overview` entry, `ExperimentalTab` and its
   import. If it stays, repoint the import to the workspace's preference hook.
5. **`docs/codebase-structure.md`:** replace the "Previews behind Settings ›
   General › Experimental" bullet with a clause in the workspace bullet:
   "`ui/overview/` the agents overview, with its rules in `model/overview/`".
6. **`docs/ARCHITECTURE.md`:** add this paragraph after the one that begins
   "The workspace (`src/desktop/workspace/`…":

   > The **agents overview** (`ui/overview/`, ⌘0 or "Agents" at the top of the
   > sidebar) takes the chat area while open, over panes left laid out beneath
   > it. It lists what waits on the person, what is working and what finished
   > unseen, filtered by Ongoing/All and a span of time (`model/overview/`),
   > with a peek at the chosen session: its live activity, this turn's steps,
   > the last thing said, and a reply pill. Its open state, selection and
   > filter are slice state, so an agent can drive it (`showOverview`,
   > `selectInOverview`, `filterOverview`). An approval it shows is on screen:
   > answered by the same `approve`/`deny` as a pane's card
   > (`onScreen`, one rule), and the conversations it shows are read and kept
   > like a pane's, bounded (`retention.overviewConversations`).
7. **ADR 238:** add the four table rows above, and mention `ui/overview/` in
   the Decision's `ui/` list. The "Naming the index" section already frees
   the word.
8. **`src/desktop/experiments/`:** delete the folder.

## Decisions for the coordinator

1. **The Settings toggle:** does "Agents overview" stay under Experimental
   until the user flips it, or go away so the view is always there?
2. **Tags in the filter menu** are a labelled section ("Tags" / "No tags
   yet"), not a submenu. The design system's submenu reported itself open
   but never drew in this window. That needs looking at in `ui/menu/` before
   tags have data. The typed seam is `SessionTag`, `AgentsFilter.tags`, and
   the bridge's `sessionTags` / `tagsOf`; a tagged summary fills it.
3. **Replying while an approval waits sets the approval aside.** This is the
   port's rule: a message moves the turn on. The pill says so. Keep it, or
   should the reply pill wait until the request is answered?
