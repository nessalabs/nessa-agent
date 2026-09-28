/**
 * An experiment, off unless turned on in Settings › General › Experimental:
 * every agent at a glance — who needs the person, for what, answerable in
 * place, and a peek at what any of them is doing without opening it — in
 * the chat area, from an "Agents" entry at the top of the sidebar or ⌘0.
 *
 * ```text
 *   workspace store ──▶ adapters/workspace-bridge.ts ──▶ model/ (glance, walk, request, peek)
 *        ▲                  │ narrow selectors; the source,              │
 *        │                  │ through the store's dependencies           ▼
 *   source's updates ◀── answerRequest / readConversation ◀── ui/ (overview, rows, peek)
 * ```
 *
 * An arrow points the way data flows. The workspace is read only through
 * `adapters/workspace-bridge.ts`; answers go to the source, which records
 * them, and come back as the workspace's own updates. The bridge says what
 * workspace API would replace its two thunks.
 *
 * A layout wraps its columns in `AgentsOverviewScope`, its pane grid in
 * `AgentsOverviewArea`, and puts `AgentsOverviewEntry` in the sidebar; each
 * draws nothing while the experiment is off.
 */
export { useAgentsOverviewPreference } from "./adapters/preference"
export {
  AgentsOverviewArea,
  AgentsOverviewEntry,
  AgentsOverviewScope,
} from "./ui/overview-scope"
