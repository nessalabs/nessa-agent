import { agentChoice, type AgentId } from "../../../onboarding/model/onboarding"
import { composerModels } from "../../model/composer-options"
import type { Handoff, HandoffAgent } from "./transcript"

/**
 * What a handoff's line says: the label on the rule, and the detail after
 * it. A switch names the agent and model it went to, as the picker names
 * them; a fork says the turns above it were copied, and how many it left out.
 */
export function handoffLine(handoff: Handoff): { label: string; meta?: string } {
  if (handoff.kind === "fork")
    return {
      label: "Forked from another chat",
      ...(handoff.omittedTurns > 0
        ? {
            meta: `${handoff.omittedTurns} earlier ${handoff.omittedTurns === 1 ? "turn" : "turns"} not shown`,
          }
        : {}),
    }
  const to = handoff.to
  return {
    label: `Switched to ${agentLabel(to)}`,
    meta: modelLabel(to),
  }
}

function agentLabel(side: HandoffAgent): string {
  const choice = side.agent === undefined ? undefined : agentChoice(side.agent as AgentId)
  return choice?.name ?? side.agent ?? "another agent"
}

/** The model's name in the catalogue, or its id when the catalogue has no such model. */
function modelLabel(side: HandoffAgent): string {
  return (
    composerModels.find((entry) => entry.modelId === side.model)?.displayName ??
    side.model
  )
}
