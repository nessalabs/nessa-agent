import { expect, it } from "vitest"
import { conversation, textContent, type Turn, type UserTurn } from "../../model"
import { emptyLocalTabs } from "../local-tabs"
import type { ConversationView } from "../view"
import { applyView } from "./apply-view"
import { beginSend, failSend } from "./send-draft"

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

/** Confirm several local submissions as queued, then stop that queue. */
function stoppedQueue(queueComplete: boolean) {
  const identities = ["one", "two", "three"]
  let tabs = emptyLocalTabs()
  for (const executionId of identities)
    tabs = beginSend(tabs, {
      conversationId: "c0",
      executionId,
      actionId: `${executionId}-action`,
      mode: "queued",
      content: textContent(executionId),
    })
  const queued = applyView(tabs.conversations[0]!, {
    ...view,
    messages: [],
    permissions: [],
    tools: [],
    queueComplete: true,
    pending: identities.map((executionId) => ({
      executionId,
      text: executionId,
      mode: "queued" as const,
    })),
  })
  expect(userReceipts(queued.turns)).toEqual([
    ["one", "queued"],
    ["two", "queued"],
    ["three", "queued"],
  ])
  // The bounded replacement retains only the last cancelled row.
  const stopped = applyView(queued, {
    ...view,
    pending: [],
    permissions: [],
    tools: [],
    queueComplete,
    messages: [
      { executionId: "three", userText: "three", parts: [], status: "cancelled" },
    ],
  })
  return { tabs: { ...tabs, conversations: [stopped] }, stopped }
}

function userReceipts(turns: Turn[]) {
  return turns
    .filter((turn): turn is UserTurn => turn.from === "user")
    .map((turn) => [turn.executionId, turn.receipt])
}

it("a complete empty queue retires confirmed queued rows the server no longer lists", () => {
  const { stopped } = stoppedQueue(true)
  expect(userReceipts(stopped.turns)).toEqual([["three", "delivered"]])
  // The retired rows are omitted history, which the same view already marks.
  expect(stopped.remote?.truncated).toBe(true)
  expect(stopped.remote?.queueComplete).toBe(true)
})

it("an accepted row the server stops listing is retired by a complete queue", () => {
  // A message the gateway reports as queued but omits from a complete pending
  // list is confirmed by the gateway, not local intent.
  const sent = beginSend(emptyLocalTabs(), {
    conversationId: "c0",
    executionId: "run",
    actionId: "run-action",
    mode: "queued",
    content: textContent("read"),
  })
  const accepted = applyView(sent.conversations[0]!, {
    ...view,
    pending: [],
    permissions: [],
    tools: [],
    messages: [{ ...view.messages[0]!, parts: [], status: "queued" }],
  })
  expect(userReceipts(accepted.turns)).toEqual([["run", "accepted"]])

  const stopped = applyView(accepted, {
    ...view,
    messages: [],
    pending: [],
    permissions: [],
    tools: [],
  })
  expect(userReceipts(stopped.turns)).toEqual([])
})

it("an incomplete queue proves nothing and keeps confirmed queued rows", () => {
  const { stopped } = stoppedQueue(false)
  expect(userReceipts(stopped.turns)).toEqual([
    ["three", "delivered"],
    ["one", "queued"],
    ["two", "queued"],
  ])
})

it("a complete queue still keeps unacknowledged local sends and local failures", () => {
  const sent = beginSend(emptyLocalTabs(), {
    conversationId: "c0",
    executionId: "sending",
    actionId: "sending-action",
    mode: "queued",
    content: textContent("in flight"),
  })
  const uncertain = failSend(
    beginSend(sent, {
      conversationId: "c0",
      executionId: "uncertain",
      actionId: "uncertain-action",
      mode: "queued",
      content: textContent("uncertain"),
    }),
    "c0",
    "uncertain",
    "offline",
  )
  const projected = applyView(uncertain.conversations[0]!, {
    ...view,
    messages: [],
    pending: [],
    permissions: [],
    tools: [],
    queueComplete: true,
  })
  expect(userReceipts(projected.turns)).toEqual([
    ["sending", "sending"],
    ["uncertain", "unknown"],
  ])
})

it("stale queued rows no longer hold the conversation in thinking after an offline rejection", () => {
  const { tabs } = stoppedQueue(true)
  const sending = beginSend(tabs, {
    conversationId: "c0",
    executionId: "next",
    actionId: "next-action",
    mode: "queued",
    content: textContent("next"),
  })
  const rejected = failSend(sending, "c0", "next", "Agent not configured", false)
  expect(rejected.conversations[0]!.phase).toBe("idle")
  expect(rejected.conversations[0]!.draft).toEqual(textContent("next"))
})
