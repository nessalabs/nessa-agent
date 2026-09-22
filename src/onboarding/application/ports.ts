/**
 * What first-run setup needs from outside itself.
 *
 * Setup asks exactly one question before it has a session — which agents could
 * actually start here — and that question is the only reason it talks to
 * anything. It is a port rather than a direct call so the surface can be driven
 * from a fake in a test, and so composition, not a UI hook, decides where the
 * answer comes from.
 */

import type {
  AgentId,
  AgentReadinessFailure,
  AgentReadinessReport,
} from "../model/onboarding"

/**
 * What came back when the runtimes were asked.
 *
 * A failure is its own shape rather than an empty report. The two are not the
 * same fact: an empty report says "no runtime here can start an agent", and a
 * failure says "nobody answered". Collapsing them made a gateway that was not
 * running look exactly like an agent Nessa does not support yet.
 */
export type AgentReadinessAnswer =
  | { readonly ok: true; readonly agents: AgentReadinessReport }
  | { readonly ok: false; readonly reason: AgentReadinessFailure }

/** Where setup learns what each agent's runtime can do. */
export interface AgentReadinessSource {
  /** Ask once. Never rejects: every way of not getting an answer is a value. */
  read(): Promise<AgentReadinessAnswer>
}

/** Agents whose API keys the native host explicitly knows how to store. */
export type ApiKeyAgent = Extract<AgentId, "claude" | "opencode">

/** The native host boundary that saves one Nessa-owned API key. */
export interface AgentApiKeySink {
  /** Save the exact key. Rejects with no secret-bearing diagnostic on failure. */
  save(agent: ApiKeyAgent, key: string): Promise<void>
}
