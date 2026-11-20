/**
 * The `DecisionSource` over the window's gateway client:
 * `client.conversation.decisions`, which the client has already checked
 * against the schema (`conversationDecisions`), read into the model's words.
 * A session's id is its conversation's (`gateway-source.ts`).
 */
import type { ConversationApi, OwnerDecision } from "@nessa/client"
import type { DecisionSource } from "../../application/ports"
import type { Decision, DecisionOutcome } from "../../model/decisions"

/** What reading decisions asks of the window's gateway client. */
export interface DecisionsClient {
  readonly conversation: Pick<ConversationApi, "decisions">
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
  }
}

export function decisionFrom({ ask, outcome }: OwnerDecision): Decision {
  return {
    ask: {
      askId: ask.askId,
      question: ask.question,
      choices: ask.choices.map(({ label, consequence }) => ({ label, consequence })),
      recommended: ask.recommended,
      reason: ask.reason,
    },
    askedAtMs: ask.askedAtMs,
    outcome: outcomeFrom(outcome),
  }
}

function outcomeFrom(outcome: OwnerDecision["outcome"]): DecisionOutcome {
  if (!outcome) return { state: "waiting" }
  if (outcome.state === "replaced") return { state: "replaced", atMs: outcome.atMs }
  return {
    state: "decided",
    text: outcome.text ?? "",
    ...(outcome.choice === undefined ? {} : { choice: outcome.choice }),
    atMs: outcome.atMs,
  }
}
