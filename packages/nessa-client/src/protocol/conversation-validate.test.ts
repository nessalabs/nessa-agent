import { describe, expect, it } from "vitest"

import { conversationView } from "./conversation-validate.js"

function view(): any {
  return {
    conversationId: "conversation",
    revision: "1",
    truncated: false,
    queueComplete: true,
    messages: [
      {
        executionId: "queued",
        userText: "hello",
        status: "queued",
        parts: [],
      },
      {
        executionId: "running",
        userText: "run",
        status: "running",
        parts: [{ offset: 0, kind: "tool", text: "", toolId: "tool" }],
      },
    ],
    pending: [{ executionId: "queued", text: "hello", mode: "queued" }],
    permissions: [],
    tools: [
      {
        executionId: "running",
        toolId: "tool",
        title: "Tool",
        status: "running",
        details: "",
        input: "{}",
      },
    ],
    capabilities: { queue: true, steer: true, resume: true, permissions: true },
  }
}

describe("conversation view agreement", () => {
  it("accepts complete matching pending and tool evidence", () => {
    expect(conversationView(view(), "conversation").revision).toBe("1")
  })

  it("rejects contradictory pending text when queue evidence is complete", () => {
    const value = view()
    value.pending[0]!.text = "different"
    expect(() => conversationView(value, "conversation")).toThrow(
      "Pending execution contradicts",
    )
  })

  it("allows pending text mismatch when bounded evidence is truncated", () => {
    const value = view()
    value.truncated = true
    value.pending[0]!.text = "bounded preview"
    expect(() => conversationView(value, "conversation")).not.toThrow()
  })

  it("rejects orphan and cross-execution tool state in complete views", () => {
    const orphan = view()
    orphan.messages[1]!.parts = []
    expect(() => conversationView(orphan, "conversation")).toThrow("orphaned")

    const mismatch = view()
    mismatch.tools[0]!.executionId = "queued"
    expect(() => conversationView(mismatch, "conversation")).toThrow()
  })

  it("allows bounded tool omissions only when the view says it is truncated", () => {
    const value = view()
    value.truncated = true
    value.tools = []
    expect(() => conversationView(value, "conversation")).not.toThrow()
  })
})
