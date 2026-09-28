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
 * A layout wraps its shell in `AgentsOverviewScope`; the shell wraps its pane
 * grid in `AgentsOverviewArea` and, while `useAgentsOverviewShown`, sets the
 * session list aside so the overview fills the content region; the sidebar
 * shows `AgentsOverviewEntry`. Each draws nothing while the experiment is off.
 */
export { useAgentsOverviewPreference } from "./adapters/preference"
export {
  AgentsOverviewArea,
  AgentsOverviewEntry,
  AgentsOverviewScope,
  useAgentsOverviewEnabled,
  useAgentsOverviewShown,
} from "./ui/overview-scope"
