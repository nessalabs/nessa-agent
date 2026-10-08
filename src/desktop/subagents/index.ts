/**
 * The desktop window's subagents (ADR 329): the agents a conversation puts
 * to work, in a panel of its own. It imports the workspace and the widget
 * contract, and neither imports it (`scripts/architecture/desktop-verticals.mjs`).
 *
 * ```text
 *   composition (main.tsx, dependencies.ts)
 *        │ sample source, joined under `sample`, or unread
 *        ▼
 *   SubagentsProvider (adapters/react/source-context.tsx)
 *        │
 *        ▼
 *   ui/plugin.tsx ── useWidget ──▶ application/widget-state.ts
 *        │                              │ preview, listing, source
 *        ├── SessionAccessory ──▶ ui/subagent-stack.tsx
 *        ▼
 *   ui/subagents-panel.tsx ── list, then one child's read-only transcript
 *        │
 *        ▼
 *   application/ports.ts: SubagentSource, joinSubagentSources
 *        │ implemented, for the sample, by adapters/in-memory/sample-source.ts
 * ```
 *
 * `model/` is the child as the panel shows it, and the tagline a seed picks.
 * `application/` is the source, the join, the widget's answer, and which
 * child a conversation has open — other verticals reach that choice through
 * `useOpenSubagent`, not the map. The header stack opens the panel without
 * choosing a child. Counts are the desktop's (`../model/counts.ts`);
 * times are the workspace's.
 */
export { SubagentsProvider } from "./adapters/react/source-context"
export {
  sampleSubagentSource,
  type SampleSubagentSchedule,
  type SampleSubagentSource,
} from "./adapters/in-memory/sample-source"
export {
  joinSubagentSources,
  joinedSubagentId,
  RepeatedSubagentSourceKey,
  sampleSubagentKey,
  splitJoinedSubagentId,
  subagentsPluginId,
  unreadSubagentSource,
  type SubagentLog,
  type SubagentRead,
  type SubagentSource,
} from "./application/ports"
export { subagentsPlugin } from "./ui/plugin"
export { useOpenSubagent } from "./ui/use-open-subagent"
