import type { ConversationEffects } from "../../application/ports"

export function scenarioEffects(scenario: "echo" | "offline"): ConversationEffects {
  return {
    async echo(text) {
      if (scenario === "offline") throw new Error("Scenario: backend offline")
      return { text }
    },
  }
}
