/**
 * The desktop window's workspace: index, sessions and chat panes, as
 * a Redux projection of one port. See ADR 238.
 *
 * ```text
 *   WorkspaceSource (application/ports.ts)
 *        │  implemented by adapters/gateway/ (the gateway's conversations)
 *        │  and adapters/in-memory/ (the sample, for fixtures and previews)
 *        ▼
 *   commands (adapters/store/commands.ts) ── thunks and plain actions
 *        │  apply use cases (application/usecases/), whose rules are model/
 *        ▼
 *   slice (adapters/store/slice.ts) ──▶ selectors, one per pane and row
 *        │
 *        ▼
 *   ui/ components, each once ──▶ ui/layouts/ that only arrange them
 *
 *   store (state.workspace.panes) ──▶ adapters/store/split-panes-source.ts
 *                                            │ SplitPanesSource
 *   adapters/dom/split-panes-drag.ts         ▼
 *   (a session's card, the side columns) ──▶ ../split-panes: <SplitPanes> in
 *                                            ui/panes/pane-grid.tsx, and
 *                                            useSplitPanesDrag and FlipScope
 *                                            in ui/layouts/workspace-shell.tsx
 *                                            │ a drop, a resize, an equalize, a fit
 *                                            ▼
 *   commands (commitDrop, resizePanes, equalizePanes, fitPanes) ──▶ slice
 * ```
 *
 * An arrow points the way data flows. The panes are split panes, a module of
 * the window's the workspace wraps (`../split-panes`, ADR 253), each showing
 * an opaque item the workspace writes and reads through one codec
 * (`model/pane-item.ts`: a session, or a widget, ADR 326): the window
 * builds one source over the store, which the grid and the drag share, and
 * the drag's options from `split-panes-drag.ts`; the module reads the layout
 * through the source and changes it only through the commands here.
 * `adapters/dom/` holds what belongs to the page rather than the product —
 * what the workspace adds to a drag (`split-panes-drag.ts`), focus, the
 * panes' room, keys, the clock's ticks — as hooks and adapters beside the
 * tree, never as state; `adapters/storage/` keeps the overview's filter
 * between launches. Widgets (ADR 326) are drawn by `../widgets`' hosts —
 * a card in a message, a widget's pane (`ui/panes/widget-pane.tsx`), the
 * window over the panes (`ui/panes/widget-window.tsx`) — in the workspace's
 * chrome, their callbacks carried out by its commands
 * (`adapters/store/widget-hosts.ts`), and Escape for the one in front by
 * `adapters/dom/widget-escape.ts`; the workspace imports widgets and no
 * plugin. The Agents overview is part of the workspace: its rules
 * in `model/overview/`, its state in the slice, its views in `ui/overview/`,
 * answering and reading through the same commands and effects as a pane.
 *
 * `store.dispatch` is the agent entry point: every command here works
 * without a renderer.
 */
export { SessionsInSidebar } from "./ui/layouts/sessions-in-sidebar"
export { ThreeColumns } from "./ui/layouts/three-columns"
export { shortcutNames, workspaceShortcuts } from "./ui/layouts/shortcuts"
export { ClockProvider } from "./adapters/dom/clock"
export { focusInFront } from "./adapters/dom/focus"
export { measureWorkspace } from "./adapters/dom/measure"
export {
  inMemorySource,
  type InMemorySource,
} from "./adapters/in-memory/in-memory-source"
export {
  seededWorkspace,
  seededWorkspaceSpec,
  SeededWorkspaceRefusal,
} from "./adapters/in-memory/seeded-workspace"
export type {
  SeededWorkspace,
  SeededWorkspaceReport,
  SeededWorkspaceSpec,
} from "./adapters/in-memory/seeded-workspace"
export {
  gatewaySource,
  type GatewayClient,
  type GatewaySource,
} from "./adapters/gateway/gateway-source"
export {
  retryBudgetSession,
  sampleAppSession,
  sampleWidgetSession,
} from "./adapters/in-memory/sample-labs"
export { workspaceEffects } from "./adapters/store/effects"
export { initialWorkspaceFrom, workspaceReducer } from "./adapters/store/slice"
export { rememberedFilter } from "./adapters/storage/remembered-filter"
export * from "./adapters/store/commands"
export { useWorkspaceDispatch, useWorkspaceSelector } from "./adapters/store/hooks"
export { selectSessionListChosen, selectSessionListing } from "./adapters/store/selectors"
export { WorkspaceSourceError } from "./application/ports"
export type {
  OutgoingMessage,
  RememberedFilter,
  WorkspaceDependencies,
  WorkspaceSource,
  WorkspaceUpdate,
} from "./application/ports"
export type { WorkspaceFailureReason } from "./model/failure"
export { elapsed, sessionTime } from "./model/time-labels"
export { ReadOnlyTranscript } from "./ui/transcript/read-only-transcript"
