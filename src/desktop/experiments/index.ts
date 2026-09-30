/**
 * Experiments: a swarm of agents hill-climbing a product against an eval,
 * shown as a widget in the conversation that started it.
 *
 * ```text
 *   ExperimentSource (application/ports.ts)
 *        │  adapters/in-memory/: a scripted swarm, played live
 *        ▼
 *   ExperimentsProvider ── useExperiment (adapters/react/)
 *        │
 *        ▼
 *   model/experiment.ts ── best so far, path to it, area summaries, verdicts
 *        │
 *        ▼
 *   ui/experiment-card.tsx      inline, in the conversation
 *   ui/experiment-surface.tsx   opened beside it: Overview · Areas · Runs,
 *                               and any run in a drawer (ui/run-detail.tsx)
 * ```
 */
export {
  ExperimentsProvider,
  useExperimentSession,
  useExperimentTitle,
} from "./adapters/react/experiments-provider"
export { liveExperimentSource } from "./adapters/in-memory/live-experiment"
export { experimentSubagents } from "./adapters/subagents/experiment-subagents"
export { sampleExperimentId } from "./adapters/in-memory/sample-experiment"
export type { ExperimentSource } from "./application/ports"
export { ExperimentCard } from "./ui/experiment-card"
export { ExperimentSurface } from "./ui/experiment-surface"
