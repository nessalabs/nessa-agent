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
 * ```
 *
 * An arrow points the way data flows. `adapters/dom/` holds what belongs to
 * the page rather than the product — motion, drag and drop, resizing, keys,
 * the clock's ticks — as hooks beside the tree, never as state.
 *
 * `store.dispatch` is the agent entry point: every command here works
 * without a renderer.
 */
export { SessionsInSidebar } from "./ui/layouts/sessions-in-sidebar"
export { ThreeColumns } from "./ui/layouts/three-columns"
export { shortcutNames, workspaceShortcuts } from "./ui/layouts/shortcuts"
export { chordLabel } from "./adapters/dom/shortcuts"
export { ClockProvider } from "./adapters/dom/clock"
export { focusComposer } from "./adapters/dom/focus"
export { measureWorkspace } from "./adapters/dom/measure"
export { inMemorySource } from "./adapters/in-memory/in-memory-source"
export { workspaceEffects } from "./adapters/store/effects"
export { workspaceReducer } from "./adapters/store/slice"
export * from "./adapters/store/commands"
export { useWorkspaceDispatch, useWorkspaceSelector } from "./adapters/store/hooks"
export { selectContentView, selectSessionListChosen } from "./adapters/store/selectors"
export { WorkspaceSourceError } from "./application/ports"
export type {
  ApprovalScope,
  Initiator,
  OutgoingMessage,
  WorkspaceDependencies,
  WorkspaceSource,
  WorkspaceUpdate,
} from "./application/ports"
export type { WorkspaceFailureReason } from "./model/failure"
