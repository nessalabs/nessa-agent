import type { ConversationEffects } from "../../application/ports"
import type { ConversationView, Submission } from "../../application/view"

/** Explicit development/test injection only; production composition always uses the gateway. */
export function scenarioEffects(scenario: "echo" | "offline"): ConversationEffects {
  const views = new Map<string, ConversationView>()
  const get = (id: string) => {
    if (scenario === "offline") throw new Error("Scenario: backend offline")
    const view = views.get(id)
    if (!view) throw new Error("Scenario conversation not found")
    return view
  }
  const send = async (input: Submission) => {
    const view = get(input.conversationId)
    if (!view.messages.some((message) => message.executionId === input.executionId)) {
      view.messages.push({
        executionId: input.executionId,
        userText: input.text,
        parts: [
          { offset: 0, kind: "thought", text: "", toolId: "" },
          { offset: 1, kind: "text", text: input.text, toolId: "" },
        ],
        status: "completed",
      })
      view.revision = String(Number(view.revision) + 1)
    }
    return { executionId: input.executionId, disposition: "queued" }
  }
  return {
    async create(conversationId) {
      if (scenario === "offline") throw new Error("Scenario: backend offline")
      if (!views.has(conversationId))
        views.set(conversationId, {
          conversationId,
          revision: "0",
          messages: [],
          pending: [],
          permissions: [],
          tools: [],
          capabilities: { queue: true, steer: false, resume: false, permissions: false },
          truncated: false,
          queueComplete: true,
        })
      return { conversationId }
    },
    async read(id) {
      return structuredClone(get(id))
    },
    send,
    steer: send,
    async reorder(id, executionIds) {
      const view = get(id)
      if (
        executionIds.length !== view.pending.length ||
        new Set(executionIds).size !== executionIds.length ||
        executionIds.some((id) => !view.pending.some((item) => item.executionId === id))
      )
        return "queue_changed"
      const ordered = executionIds.flatMap((id) =>
        view.pending.filter((item) => item.executionId === id),
      )
      let ordinary = false
      for (const item of ordered) {
        if (item.mode === "queued") ordinary = true
        else if (ordinary) return "priority_conflict"
      }
      if (view.pending.every((item, index) => item.executionId === executionIds[index]))
        return "unchanged"
      view.pending = ordered
      view.revision = String(Number(view.revision) + 1)
      return "applied"
    },
    async remove() {},
    async answer() {},
    async cancel() {},
    async close(id) {
      const view = get(id)
      view.messages = view.messages.map((message) =>
        message.status === "running" || message.status === "queued"
          ? { ...message, status: "cancelled" }
          : message,
      )
      view.pending = []
      view.permissions = []
    },
  }
}
