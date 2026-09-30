import type { SessionSubagents } from "../model/subagent"

/**
 * Where a conversation's subagents come from. Today an experiment's swarm is
 * read as its conversation's subagents (`experiments/adapters/subagents`);
 * the gateway's own subagents join it later. `forSession` answers from what
 * the source already holds, so a view can read it on every render.
 */
export interface SubagentSource {
  forSession(sessionId: string): SessionSubagents | undefined
  subscribe(listener: () => void): () => void
  /** Says something to one subagent, in its own conversation. */
  send(sessionId: string, subagentId: string, text: string): void
}
