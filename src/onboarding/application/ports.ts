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

/** Confirmed secure-store effect and its independent audit delivery. */
export type AgentApiKeySave =
  { readonly status: "saved" } | { readonly status: "saved-audit-failed" }

export type AgentApiKeyAuditStatus = "recorded" | "failed" | "unknown"

export type AgentApiKeySaveRejectionReason =
  | "untrusted-caller"
  | "unsupported-agent"
  | "invalid-credential"
  | "store-unavailable"
  | "audit-unavailable"

export type AgentApiKeySaveRejection =
  | { readonly reason: Exclude<AgentApiKeySaveRejectionReason, "invalid-credential"> }
  | {
      readonly reason: "invalid-credential"
      readonly auditStatus: Exclude<AgentApiKeyAuditStatus, "unknown">
    }

/** A validated native refusal that proves no secure-store replacement occurred. */
export class AgentApiKeySaveRejected extends Error {
  constructor(readonly rejection: AgentApiKeySaveRejection) {
    super("The native credential save was refused")
    this.name = "AgentApiKeySaveRejected"
  }

  get reason() {
    return this.rejection.reason
  }

  get auditStatus() {
    return this.rejection.reason === "invalid-credential"
      ? this.rejection.auditStatus
      : undefined
  }
}

/** A save whose secure-store effect could not be classified at the native boundary. */
export class AgentApiKeySaveUncertain extends Error {
  constructor(readonly auditStatus: AgentApiKeyAuditStatus) {
    super("The native credential save outcome is uncertain")
    this.name = "AgentApiKeySaveUncertain"
  }
}

/** The native host boundary that saves one Nessa-owned API key. */
export interface AgentApiKeySink {
  /** Save the exact key. Rejects with no secret-bearing diagnostic on failure. */
  save(agent: ApiKeyAgent, key: string): Promise<AgentApiKeySave>
}
