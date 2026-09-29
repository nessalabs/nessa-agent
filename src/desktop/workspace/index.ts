/**
 * The desktop window's workspace: index, sessions and chat panes, as
 * a Redux projection of one port. See ADR 238.
 *
 * ```text
 *   WorkspaceSource (application/ports.ts)
 *        │  implemented by adapters/in-memory/ today, the gateway later
 *        ▼
 *   commands (adapters/store/commands.ts) ── thunks and plain actions
 *        │  apply use cases (application/usecases/), whose rules are model/
 *        ▼
 *   slice (adapters/store/slice.ts) ──▶ selectors, one per pane and row
 *        │
 *        ▼
 *   ui/ components, each once ──▶ ui/layouts/ that only arrange them
 *
 *   slice ──▶ adapters/store/split-panes-source.ts ──▶ split-panes (../split-panes)
 *                                                       the grid, the drag, FLIP
 * ```
 *
 * An arrow points the way data flows. The panes are split panes, a module of
 * the window's the workspace wraps (`../split-panes`, ADR 253): it reads the
 * layout through the workspace's source and changes it only through the
 * commands here. `adapters/dom/` holds what belongs to the page rather than
 * the product — what the workspace adds to a drag (`split-panes-drag.ts`),
 * focus, the panes' room, keys, the clock's ticks — as hooks and adapters
 * beside the tree, never as state;
 * `adapters/storage/` keeps the overview's filter between launches. The Agents overview
 * is part of the workspace: its rules in `model/overview/`, its state in the
 * slice, its views in `ui/overview/`, answering and reading through the same
 * commands and effects as a pane.
 *
 * `store.dispatch` is the agent entry point: every command here works
 * without a renderer.
 */
export { SessionsInSidebar } from "./ui/layouts/sessions-in-sidebar"
export { ThreeColumns } from "./ui/layouts/three-columns"
export { shortcutNames, workspaceShortcuts } from "./ui/layouts/shortcuts"
export { ClockProvider } from "./adapters/dom/clock"
export { focusComposer } from "./adapters/dom/focus"
export { measureWorkspace } from "./adapters/dom/measure"
export { inMemorySource } from "./adapters/in-memory/in-memory-source"
export { workspaceEffects } from "./adapters/store/effects"
export { initialWorkspaceFrom, workspaceReducer } from "./adapters/store/slice"
export { rememberedFilter } from "./adapters/storage/remembered-filter"
export * from "./adapters/store/commands"
export { useWorkspaceDispatch, useWorkspaceSelector } from "./adapters/store/hooks"
export { selectSessionListChosen } from "./adapters/store/selectors"
export { WorkspaceSourceError } from "./application/ports"
export type {
  OutgoingMessage,
  RememberedFilter,
  WorkspaceDependencies,
  WorkspaceSource,
  WorkspaceUpdate,
} from "./application/ports"
export type { WorkspaceFailureReason } from "./model/failure"
