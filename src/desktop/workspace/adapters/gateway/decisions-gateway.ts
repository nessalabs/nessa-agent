/**
 * The `DecisionSource` over the window's gateway client:
 * `client.conversation.decisions`, which the client has already checked
 * against the schema (`conversationDecisions`), read into the model's words.
 * A session's id is its conversation's (`gateway-source.ts`).
 */
import type { ConversationApi, OwnerDecision } from "@nessa/client"
import type { DecisionSource } from "../../application/ports"
import type { Decision, DecisionOutcome } from "../../model/decisions"
import { askFrom } from "./gateway-views"

/** What reading decisions asks of the window's gateway client. */
export interface DecisionsClient {
  readonly conversation: Pick<ConversationApi, "decisions" | "pauseAutoPick">
}

export function decisionsGateway({
  connected,
}: {
  connected: () => Promise<DecisionsClient>
}): DecisionSource {
  return {
    async decisions(sessionId) {
      const { decisions } = await (await connected()).conversation.decisions(sessionId)
      return decisions.map(decisionFrom)
    },
    async pauseAutoPick(sessionId, askId, paused) {
      return (await connected()).conversation.pauseAutoPick(sessionId, askId, paused)
    },
  }
}

export function decisionFrom({
  ask,
  outcome,
  autoPickPaused,
  autoPickDueAtMs,
}: OwnerDecision): Decision {
  const autoPick: Decision["autoPick"] = autoPickPaused
    ? { state: "paused" }
    : autoPickDueAtMs === undefined
      ? undefined
      : { state: "due", atMs: autoPickDueAtMs }
  return {
    ask: askFrom(ask),
    askedAtMs: ask.askedAtMs,
    outcome: outcomeFrom(outcome),
    ...(autoPick ? { autoPick } : {}),
  }
}

function outcomeFrom(outcome: OwnerDecision["outcome"]): DecisionOutcome {
  if (!outcome) return { state: "waiting" }
  if (outcome.state === "replaced") return { state: "replaced", atMs: outcome.atMs }
  return {
    state: "decided",
    text: outcome.text ?? "",
    ...(outcome.choice === undefined ? {} : { choice: outcome.choice }),
    ...(outcome.auto ? { auto: true as const } : {}),
    atMs: outcome.atMs,
  }
}
