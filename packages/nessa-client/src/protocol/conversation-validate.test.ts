import { describe, expect, it } from "vitest"

import {
  conversationId,
  conversationMutation,
  conversationReceipt,
  conversationReorder,
  conversationView,
} from "./conversation-validate.js"

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

  it("rejects unknown fields at the view and nested schema boundaries", () => {
    const mutations = [
      (value: any) => (value.extra = true),
      (value: any) => (value.messages[0].extra = true),
      (value: any) => (value.messages[1].parts[0].extra = true),
      (value: any) => (value.pending[0].extra = true),
      (value: any) => (value.tools[0].extra = true),
      (value: any) => (value.capabilities.extra = true),
    ]
    for (const mutate of mutations) {
      const value = view()
      mutate(value)
      expect(() => conversationView(value, "conversation")).toThrow("unknown fields")
    }

    const permission = view()
    permission.permissions = [
      {
        executionId: "running",
        permissionId: "permission",
        toolId: "tool",
        title: "Review",
        toolName: "write_file",
        argumentsJson: "{}",
        options: [{ id: "allow", label: "Allow", extra: true }],
      },
    ]
    expect(() => conversationView(permission, "conversation")).toThrow("unknown fields")
  })

  it("rejects unknown fields in receipts and control results", () => {
    expect(() => conversationId({ conversationId: "c", extra: true }, "c")).toThrow()
    expect(() =>
      conversationReceipt({ executionId: "e", disposition: "queued", extra: true }, "e"),
    ).toThrow()
    expect(() =>
      conversationMutation({ requestId: "r", applied: true, extra: true }, "r"),
    ).toThrow()
    expect(() =>
      conversationReorder({ requestId: "r", outcome: "applied", extra: true }, "r"),
    ).toThrow()
  })
})
