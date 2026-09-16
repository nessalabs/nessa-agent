import { expect, it } from "vitest"
import { conversation, textContent } from "../../model"
import type { ConversationView } from "../view"
import { applyView } from "./apply-view"

const view: ConversationView = {
  conversationId: "server",
  revision: "opaque-revision",
  truncated: true,
  queueComplete: true,
  messages: [
    {
      executionId: "run",
      userText: "read",
      parts: [
        { offset: 0, kind: "thought", text: "reasoning", toolId: "" },
        { offset: 1, kind: "text", text: "streaming text", toolId: "" },
      ],
      status: "running",
    },
  ],
  pending: [{ executionId: "next", text: "follow up", mode: "queued" }],
  permissions: [
    {
      executionId: "run",
      permissionId: "review",
      toolId: "tool",
      title: "Read",
      toolName: "Read",
      argumentsJson: '{"path":"/exact/path"}',
      options: [
        { id: "allow", label: "Allow once" },
        { id: "deny", label: "Deny" },
      ],
    },
  ],
  tools: [
    {
      executionId: "run",
      input: "",
      details: "",
      toolId: "tool",
      title: "Read",
      status: "pending",
    },
  ],
  capabilities: { queue: true, steer: true, resume: true, permissions: true },
}
it("projects exact review and tool targets while preserving the next unsent draft", () => {
  const current = {
    ...conversation("tab"),
    serverConversationId: "server",
    draft: textContent("unsent"),
  }
  const projected = applyView(current, view)
  expect(projected.phase).toBe("streaming")
  expect(projected.draft).toEqual(textContent("unsent"))
  expect(projected.turns[1]).toMatchObject({
    text: "streaming text",
    thought: "reasoning",
    status: "running",
  })
  expect(projected.remote?.permissions).toEqual(view.permissions)
  expect(projected.remote?.tools).toEqual(view.tools)
  expect(projected.remote?.truncated).toBe(true)
})
it("keeps error and status separate from actual assistant text, and handles empty permission views honestly", () => {
  const projected = applyView(conversation("tab"), {
    ...view,
    messages: [
      {
        ...view.messages[0]!,
        parts: [
          { offset: 0, kind: "thought", text: "", toolId: "" },
          { offset: 1, kind: "text", text: "", toolId: "" },
        ],
        status: "failed",
        error: "Provider failed",
      },
    ],
    pending: [],
    permissions: [],
    permissionViewError: "Review exceeds safe view limit",
  })
  expect(projected.phase).toBe("idle")
  expect(projected.turns[1]).toMatchObject({
    from: "assistant",
    text: "",
    status: "Provider failed",
  })
  expect(projected.remote?.permissions).toEqual([])
  expect(projected.remote?.permissionViewError).toBe("Review exceeds safe view limit")
})

it("replaces server-only queue rows when the next complete view removes them", () => {
  const first = applyView(conversation("tab"), view)
  const stopped = applyView(first, {
    ...view,
    messages: [],
    pending: [],
    permissions: [],
    tools: [],
  })
  expect(stopped.turns).toEqual([])
})

it("labels confirmed waiting work as queued without inventing an assistant message", () => {
  const projected = applyView(conversation("tab"), {
    ...view,
    pending: [{ executionId: "run", text: "read", mode: "queued" }],
    messages: [{ ...view.messages[0]!, parts: [], status: "queued" }],
  })
  expect(projected.turns).toHaveLength(1)
  expect(projected.turns[0]).toMatchObject({ from: "user", receipt: "queued" })
})

it("keeps selected work in the transcript while the first output is still pending", () => {
  const projected = applyView(conversation("tab"), {
    ...view,
    pending: [],
    messages: [{ ...view.messages[0]!, parts: [], status: "queued" }],
  })
  expect(projected.turns[0]).toMatchObject({ from: "user", receipt: "accepted" })
  expect(projected.remote?.pending).toEqual([])
})
