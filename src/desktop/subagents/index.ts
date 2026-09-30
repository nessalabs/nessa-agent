/**
 * Subagents: the agents a conversation has put to work, in a panel of the
 * conversation's own — every one, what it is doing now, and what it has done.
 *
 * ```text
 *   SubagentSource (application/ports.ts)
 *        │  experiments/adapters/subagents: an experiment's swarm, today
 *        ▼
 *   SubagentsProvider ── useSessionSubagents, useFocusedSubagent (adapters/react/)
 *        │
 *        ▼
 *   model/subagent.ts ── states, tags, order
 *        │
 *        ▼
 *   ui/subagents-panel.tsx   the list, and one subagent in detail
 *   ui/subagent-stack.tsx    a conversation's subagents in a glance, for its header
 * ```
 *
 * The panel is a widget (`../widgets`), opened in a pane of its own beside
 * the conversation; which subagent it shows is held by the provider, so a
 * click anywhere in the window can open it on one.
 */
export {
  SubagentsProvider,
  useFocusSubagent,
  useFocusedSubagent,
  useSessionSubagents,
  useSubagentTrail,
} from "./adapters/react/subagents-provider"
export type { SubagentSource } from "./application/ports"
export type {
  SessionSubagents,
  Subagent,
  SubagentTag,
  SubagentState,
  SubagentWork,
} from "./model/subagent"
export { SubagentsPanel } from "./ui/subagents-panel"
export { SubagentStack } from "./ui/subagent-stack"
