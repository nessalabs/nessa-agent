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
      attachments: [],
      files: [],
      parts: [
        { offset: 0, kind: "thought", text: "reasoning", toolId: "" },
        { offset: 1, kind: "text", text: "streaming text", toolId: "" },
      ],
      status: "running",
    },
  ],
  pending: [
    {
      executionId: "next",
      text: "follow up",
      attachments: [],
      files: [],
      mode: "queued",
    },
  ],
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
  capabilities: {
    queue: true,
    steer: true,
    resume: true,
    permissions: true,
    imageInput: false,
  },
  lifecycle: { phase: "attached" },
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
    pending: [
      { executionId: "run", text: "read", attachments: [], files: [], mode: "queued" },
    ],
    messages: [{ ...view.messages[0]!, parts: [], status: "queued" }],
  })
  expect(projected.turns).toHaveLength(1)
  expect(projected.turns[0]).toMatchObject({ from: "user", receipt: "queued" })
})

it("shows provider attachment as starting rather than model thinking", () => {
  const projected = applyView(conversation("tab"), {
    ...view,
    lifecycle: { phase: "starting" },
    messages: [{ ...view.messages[0]!, parts: [], status: "queued" }],
    pending: [
      { executionId: "run", text: "read", attachments: [], files: [], mode: "queued" },
    ],
  })
  expect(projected.phase).toBe("starting")
  expect(projected.remote?.lifecycle).toEqual({ phase: "starting" })
})

it("does not label queued work as model thinking after attachment failed", () => {
  const projected = applyView(conversation("tab"), {
    ...view,
    lifecycle: {
      phase: "failed",
      failure: { code: "provider", message: "Provider did not start" },
    },
    messages: [{ ...view.messages[0]!, parts: [], status: "queued" }],
    pending: [
      { executionId: "run", text: "read", attachments: [], files: [], mode: "queued" },
    ],
  })
  expect(projected.phase).toBe("idle")
  expect(projected.remote?.lifecycle.failure?.code).toBe("provider")
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
      attachments: [],
      files: [],
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
      {
        executionId: "three",
        userText: "three",
        attachments: [],
        files: [],
        parts: [],
        status: "cancelled",
      },
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
    { kind: "uncertain" },
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

/** A send refused before admission: the message was not taken, so the draft is back. */
const refused = { kind: "refused", reupload: false } as const

/**
 * The sentence and the typed reason behind it describe the same failure, so a
 * view that retires one retires the other. A notice left branching on a reason
 * the conversation no longer reports would name a cause with nothing to show.
 */
it("retires a refusal's reason with its sentence, and keeps both while the failed turn stands", () => {
  const sent = beginSend(emptyLocalTabs(), {
    conversationId: "c0",
    executionId: "run",
    actionId: "run-action",
    mode: "queued",
    content: textContent("read"),
  })
  const failed = failSend(
    sent,
    "c0",
    "run",
    "The agent was still starting and ran out of time.",
    refused,
    "agent-startup-deadline",
  ).conversations[0]!
  expect(failed.failure).toBe("agent-startup-deadline")
  // A view that says nothing about this execution leaves the failure standing.
  const unrelated = applyView(failed, {
    ...view,
    messages: [],
    pending: [],
    permissions: [],
    tools: [],
  })
  expect(unrelated.error).toBe(failed.error)
  expect(unrelated.failure).toBe("agent-startup-deadline")
  // A view that has the turn after all supersedes what the local failure said.
  const seen = applyView(failed, {
    ...view,
    pending: [],
    permissions: [],
    tools: [],
    messages: [{ ...view.messages[0]!, parts: [], status: "queued" }],
  })
  expect(seen.error).toBeUndefined()
  expect(seen.failure).toBeUndefined()
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
  const rejected = failSend(sending, "c0", "next", "Agent not configured", refused)
  expect(rejected.conversations[0]!.phase).toBe("idle")
  expect(rejected.conversations[0]!.draft).toEqual(textContent("next"))
})

const DIGEST = `sha256:${"ab".repeat(32)}`
it("shows a turn it never held the bytes for as text plus image references", () => {
  const picture = { digest: DIGEST, mimeType: "image/png" as const, size: 2048 }
  const applied = applyView(emptyLocalTabs().conversations[0]!, {
    ...view,
    tools: [],
    permissions: [],
    messages: [
      {
        executionId: "captioned",
        userText: "what is this?",
        attachments: [picture],
        files: [],
        parts: [],
        status: "completed",
      },
      // An image-only turn still has content, so the transcript has something to paint.
      {
        executionId: "bare",
        userText: "",
        attachments: [picture, { ...picture, mimeType: "image/jpeg" }],
        files: [],
        parts: [],
        status: "queued",
      },
    ],
    pending: [
      {
        executionId: "bare",
        text: "",
        attachments: [picture, { ...picture, mimeType: "image/jpeg" }],
        files: [],
        mode: "queued",
      },
      {
        executionId: "waiting",
        text: "",
        attachments: [picture],
        files: [],
        mode: "queued",
      },
    ],
  })
  const users = applied.turns.filter((turn) => turn.from === "user")
  expect(users.map((turn) => turn.content)).toEqual([
    [
      { type: "text", text: "what is this?" },
      { type: "image-reference", ...picture },
    ],
    [
      { type: "image-reference", ...picture },
      { type: "image-reference", ...picture, mimeType: "image/jpeg" },
    ],
    [{ type: "image-reference", ...picture }],
  ])
  expect(applied.remote?.pending[1]?.attachments).toEqual([picture])
  expect(applied.remote?.capabilities.imageInput).toBe(false)
})

it("keeps a sent turn's local previews when the gateway echoes it by reference", () => {
  const file = {
    type: "file" as const,
    id: "f",
    name: "finder.png",
    mimeType: "image/png",
    size: 2048,
    previewUrl: "blob:finder",
    upload: {
      status: "stored" as const,
      image: { digest: DIGEST, mimeType: "image/png" as const, size: 2048 },
    },
    path: null,
  }
  const base = emptyLocalTabs()
  const tabs = {
    ...base,
    conversations: [{ ...base.conversations[0]!, draft: [file] }],
  }
  const sent = beginSend(tabs, {
    conversationId: "c0",
    executionId: "mine",
    actionId: "action",
    mode: "queued",
    content: [file],
  })
  const applied = applyView(sent.conversations[0]!, {
    ...view,
    tools: [],
    permissions: [],
    pending: [],
    messages: [
      {
        executionId: "mine",
        userText: "",
        attachments: [{ digest: DIGEST, mimeType: "image/png", size: 2048 }],
        files: [],
        parts: [],
        status: "running",
      },
    ],
  })
  expect(applied.turns[0]).toMatchObject({ from: "user", content: [file] })
})
