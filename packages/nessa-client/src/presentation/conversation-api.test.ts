import { NessaRpcError } from "../application/rpc-error.js"
import { expect, it, vi } from "vitest"
import { createConversationApi } from "./conversation-api.js"
import {
  NessaConversationMutationError,
  NessaConversationControlError,
} from "../application/conversation-mutation-error.js"
import { ConversationErrorCode, type ConversationView } from "../generated/product.js"

const conversationId = "00000000-0000-4000-8000-000000000001"

const view: ConversationView = {
  conversationId,
  revision: "opaque-1",
  messages: [],
  pending: [],
  permissions: [],
  tools: [],
  capabilities: { queue: true, steer: true, resume: true, permissions: true },
  truncated: false,
  queueComplete: true,
}
it("owns immutable message and action identities through an uncertain retry", async () => {
  const request = vi
    .fn()
    .mockRejectedValueOnce(new Error("connection lost"))
    .mockResolvedValueOnce({ executionId: "execution", disposition: "queued" })
  const api = createConversationApi({ request }, () => "unused")
  const options = { executionId: "execution", requestId: "action" }
  const error = await api.send(conversationId, "hello", options).catch((error) => error)
  options.executionId = "edited-id"
  options.requestId = "edited-action"
  expect(error).toBeInstanceOf(NessaConversationMutationError)
  expect(error).toMatchObject({
    conversationId: conversationId,
    executionId: "execution",
    requestId: "action",
  })
  expect(request).toHaveBeenCalledTimes(1)
  expect(await error.retry()).toEqual({
    executionId: "execution",
    disposition: "queued",
    requestId: "action",
  })
  expect(request.mock.calls[0]).toEqual(request.mock.calls[1])
  expect(request.mock.calls[1][1]).toEqual({
    conversationId: conversationId,
    executionId: "execution",
    requestId: "action",
    text: "hello",
  })
})
it("generates distinct identities per intentional message, retaining each on retry", async () => {
  let next = 0
  const request = vi.fn(async (_method: string, params: unknown) => ({
    executionId: (params as { executionId: string }).executionId,
    disposition: "queued",
  }))
  const api = createConversationApi({ request }, () => `id-${++next}`)
  expect(await api.send(conversationId, "same text")).toMatchObject({
    executionId: "id-1",
    requestId: "id-2",
  })
  expect(await api.send(conversationId, "same text")).toMatchObject({
    executionId: "id-3",
    requestId: "id-4",
  })
})
it("retains generated conversation identity when creation acknowledgement is lost", async () => {
  const ids = [conversationId, "create-request"]
  let next = 0
  const request = vi
    .fn()
    .mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValueOnce({ conversationId })
  const api = createConversationApi({ request }, () => ids[next++]!)
  const error = await api.create().catch((error) => error)
  expect(error.conversationId).toBe(conversationId)
  expect(await error.retry()).toEqual({ conversationId })
  expect(request.mock.calls[0]).toEqual(request.mock.calls[1])
  expect(next).toBe(2)
})
it("rejects terminal executions that remain pending or actionable", async () => {
  const terminal = {
    executionId: "finished",
    userText: "done",
    parts: [],
    status: "completed",
  } as const
  const permission = {
    executionId: "finished",
    permissionId: "review",
    toolId: "tool",
    title: "Review",
    toolName: "shell",
    argumentsJson: "{}",
    options: [{ id: "deny", label: "Deny" }],
  }
  const request = vi.fn()
  const api = createConversationApi({ request }, () => "id")
  for (const invalid of [
    {
      ...view,
      messages: [terminal],
      pending: [{ executionId: "finished", text: "done", mode: "queued" }],
    },
    { ...view, messages: [terminal], permissions: [permission] },
  ]) {
    request.mockResolvedValueOnce(invalid)
    await expect(api.read(conversationId)).rejects.toThrow()
  }
})
it("routes every control with stable action and target identities", async () => {
  const request = vi.fn(async (_method: string, params: unknown) => ({
    requestId: (params as { requestId: string }).requestId,
    applied: true,
  }))
  const api = createConversationApi({ request }, () => "action")
  await api.remove(conversationId, "execution")
  await api.answer(conversationId, "execution", "review", "allow")
  await api.cancel(conversationId, "execution", "review", "user withdrew")
  await api.close(conversationId)
  expect(request.mock.calls.map((call) => call[0])).toEqual([
    "conversation.remove",
    "conversation.answer",
    "conversation.cancel",
    "conversation.close",
  ])
  expect(request.mock.calls[1][1]).toEqual({
    conversationId: conversationId,
    requestId: "action",
    executionId: "execution",
    permissionId: "review",
    optionId: "allow",
  })
})
it("accepts bounded full replacement views and rejects mismatched identities or invalid choices", async () => {
  const request = vi.fn().mockResolvedValue(view)
  const api = createConversationApi({ request }, () => "id")
  expect(await api.read(conversationId)).toEqual(view)
  for (const invalid of [
    { ...view, conversationId: "another" },
    { ...view, capabilities: { ...view.capabilities, steer: "true" } },
    {
      ...view,
      messages: [
        {
          executionId: "e",
          userText: "hi",
          parts: [
            { offset: 0, kind: "thought", text: "", toolId: "" },
            { offset: 1, kind: "text", text: "", toolId: "" },
          ],
          status: "maybe",
        },
      ],
    },
    {
      ...view,
      permissions: [
        {
          executionId: "e",
          permissionId: "p",
          toolId: "t",
          title: "run",
          toolName: "shell",
          argumentsJson: "{}",
          options: [
            { id: "same", label: "Allow" },
            { id: "same", label: "Deny" },
          ],
        },
      ],
    },
  ]) {
    request.mockResolvedValueOnce(invalid)
    await expect(api.read(conversationId)).rejects.toThrow()
  }
})
it("rejects duplicate identities and impossible steering order at the response boundary", async () => {
  const message = {
    executionId: "first",
    userText: "hello",
    parts: [],
    status: "completed",
  } as const
  const permission = {
    executionId: "first",
    permissionId: "permission",
    toolId: "tool",
    title: "Run",
    toolName: "shell",
    argumentsJson: "{}",
    options: [{ id: "allow", label: "Allow" }],
  }
  const tool = {
    executionId: "first",
    toolId: "tool",
    title: "Run",
    status: "completed",
    input: "{}",
    details: "done",
  }
  const request = vi.fn()
  const api = createConversationApi({ request }, () => "id")
  for (const invalid of [
    { ...view, messages: [message, message] },
    {
      ...view,
      pending: [
        { executionId: "waiting", text: "one", mode: "queued" },
        { executionId: "waiting", text: "two", mode: "steering" },
      ],
    },
    { ...view, permissions: [permission, permission] },
    { ...view, tools: [tool, tool] },
    {
      ...view,
      messages: [
        {
          executionId: "steer",
          userText: "correction",
          parts: [],
          status: "injected",
          steeringTarget: "later",
          steeringOffset: 0,
        },
        { ...message, executionId: "later" },
      ],
    },
    {
      ...view,
      messages: [
        {
          executionId: "self",
          userText: "correction",
          parts: [],
          status: "injected",
          steeringTarget: "self",
          steeringOffset: 0,
        },
      ],
    },
    {
      ...view,
      messages: [
        {
          executionId: "steer",
          userText: "correction",
          parts: [],
          status: "injected",
        },
      ],
    },
    {
      ...view,
      messages: [
        {
          ...message,
          steeringTarget: "earlier",
          steeringOffset: 0,
        },
      ],
      truncated: true,
    },
  ]) {
    request.mockResolvedValueOnce(invalid)
    await expect(api.read(conversationId)).rejects.toThrow()
  }
})

it("accepts bounded omissions and the intentional pending-message overlap", async () => {
  const request = vi.fn().mockResolvedValue({
    ...view,
    truncated: true,
    queueComplete: false,
    messages: [
      {
        executionId: "steer",
        userText: "correction",
        parts: [],
        status: "injected",
        steeringTarget: "omitted-target",
        steeringOffset: 2,
      },
      {
        executionId: "waiting",
        userText: "next",
        parts: [],
        status: "queued",
      },
    ],
    pending: [{ executionId: "waiting", text: "next", mode: "queued" }],
    permissions: [
      {
        executionId: "omitted-execution",
        permissionId: "review",
        toolId: "omitted-tool",
        title: "Review",
        toolName: "shell",
        argumentsJson: "{}",
        options: [{ id: "deny", label: "Deny" }],
      },
    ],
    tools: [
      {
        executionId: "omitted-execution",
        toolId: "omitted-tool",
        title: "Run",
        status: "running",
        input: "{}",
        details: "",
      },
    ],
  })
  const api = createConversationApi({ request }, () => "id")
  await expect(api.read(conversationId)).resolves.toMatchObject({
    truncated: true,
    queueComplete: false,
  })
})
it("rejects a response acknowledging a different execution or action", async () => {
  const request = vi
    .fn()
    .mockResolvedValue({ executionId: "other", disposition: "queued" })
  const api = createConversationApi({ request }, () => "id")
  await expect(
    api.steer(conversationId, "correction", { executionId: "expected" }),
  ).rejects.toBeInstanceOf(NessaConversationMutationError)
  request.mockResolvedValue({ requestId: "other", applied: true })
  await expect(
    api.close(conversationId, { requestId: "expected" }),
  ).rejects.toBeInstanceOf(NessaConversationControlError)
})

it("never offers a replay for controls whose acknowledgement was lost", async () => {
  const request = vi.fn().mockRejectedValue(new Error("connection lost after effect"))
  const api = createConversationApi({ request }, () => "action")
  const controls = [
    () => api.close(conversationId),
    () => api.remove(conversationId, "execution"),
    () => api.answer(conversationId, "execution", "permission", "allow"),
    () => api.cancel(conversationId, "execution", "permission", "withdrawn"),
  ]
  for (const control of controls) {
    const error = await control().catch((error: unknown) => error)
    expect(error).toBeInstanceOf(NessaConversationControlError)
    expect(error).toMatchObject({ uncertain: true, requestId: "action" })
    expect(error).not.toHaveProperty("retry")
  }
  expect(request).toHaveBeenCalledTimes(4)
})

it("preserves every typed permission selection state independently of its diagnostic", async () => {
  const request = vi.fn()
  const api = createConversationApi({ request }, () => "action")
  for (const permissionSelection of ["pending", "consumed", "unknown"] as const) {
    request.mockRejectedValueOnce(
      new NessaRpcError("audit_unavailable", "audit_unavailable", {
        selectionState: permissionSelection,
      }),
    )
    const error = await api
      .answer(conversationId, "execution", "permission", "allow")
      .catch((error: unknown) => error)
    expect(error).toBeInstanceOf(NessaConversationControlError)
    expect(error).toMatchObject({
      permissionSelection,
      uncertain: permissionSelection !== "pending",
    })
  }
})

it("does not infer permission selection from an error code or malformed details", async () => {
  const request = vi
    .fn()
    .mockRejectedValueOnce(new NessaRpcError("audit_unavailable", "consumed"))
    .mockRejectedValueOnce(
      new NessaRpcError("audit_unavailable", "audit_unavailable", {
        selectionState: "consumed-ish",
      }),
    )
    .mockRejectedValueOnce(
      new NessaRpcError("audit_unavailable", "audit_unavailable", {
        selectionState: "consumed",
        contradictory: true,
      }),
    )
  const api = createConversationApi({ request }, () => "action")
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const error = await api
      .answer(conversationId, "execution", "permission", "allow")
      .catch((error: unknown) => error)
    expect(error).toMatchObject({ permissionSelection: undefined })
  }
})

it("never treats permission details as state for a different control", async () => {
  const request = vi.fn().mockRejectedValue(
    new NessaRpcError("audit_unavailable", "audit_unavailable", {
      selectionState: "consumed",
    }),
  )
  const api = createConversationApi({ request }, () => "action")
  const error = await api
    .remove(conversationId, "execution")
    .catch((error: unknown) => error)
  expect(error).toMatchObject({ permissionSelection: undefined, uncertain: true })
})

it("reports missing provider as a known rejection rather than unknown delivery", async () => {
  const request = vi
    .fn()
    .mockRejectedValue(new NessaRpcError("agent_not_configured", "agent_not_configured"))
  const ids = [conversationId, "identity"]
  let next = 0
  const api = createConversationApi({ request }, () => ids[next++]!)
  const error = await api.create().catch((error) => error)
  expect(error).toBeInstanceOf(NessaConversationMutationError)
  expect(error.uncertain).toBe(false)
  expect(error.message).toContain("started without an agent")
  // The remedy is named, not just the symptom.
  expect(error.message).toContain("just server")
})
it("reports invalid requests as known pre-admission rejections", async () => {
  const request = vi
    .fn()
    .mockRejectedValue(new NessaRpcError("invalid_request", "invalid_request"))
  const api = createConversationApi({ request }, () => "identity")
  const sendError = await api.send(conversationId, "hello").catch((error) => error)
  const closeError = await api.close(conversationId).catch((error) => error)
  expect(sendError).toMatchObject({ uncertain: false })
  expect(closeError).toMatchObject({ uncertain: false })
})

it("says the agent was still starting and that the same command may be retried", async () => {
  const request = vi
    .fn()
    .mockRejectedValue(
      new NessaRpcError("agent_startup_deadline", "agent_startup_deadline"),
    )
  const api = createConversationApi({ request }, () => "identity")
  const error = await api.send(conversationId, "hello").catch((error) => error)
  expect(error).toBeInstanceOf(NessaConversationMutationError)
  expect(error.code).toBe(ConversationErrorCode.AgentStartupDeadline)
  // Startup runs before any input reaches the provider: this is a rejection,
  // not an unknown delivery.
  expect(error.uncertain).toBe(false)
  expect(error.message).toContain("still starting")
  expect(error.message).toContain("Retrying is expected to work")
})

it("leaves an unrecognized gateway code untyped instead of guessing a meaning", async () => {
  const request = vi
    .fn()
    .mockRejectedValue(new NessaRpcError("invented_code", "invented_code"))
  const api = createConversationApi({ request }, () => "identity")
  const error = await api.send(conversationId, "hello").catch((error) => error)
  expect(error.code).toBeUndefined()
  expect(error.uncertain).toBe(true)
  expect(error.message).toBe("Conversation command failed")
})

it("enforces canonical conversation and UTF-8 byte limits before admission", async () => {
  const request = vi.fn().mockImplementation((_method, params) =>
    Promise.resolve({
      executionId: (params as { executionId: string }).executionId,
      disposition: "queued",
    }),
  )
  const api = createConversationApi({ request }, () => "request")
  expect(() => api.send("conversation", "hello")).toThrow(TypeError)
  expect(() => api.send(conversationId, "😀".repeat(2049))).toThrow(TypeError)
  expect(() =>
    api.send(conversationId, "hello", { executionId: "😀".repeat(65) }),
  ).toThrow(TypeError)
  expect(() => api.send(conversationId, "hello", { requestId: "😀".repeat(65) })).toThrow(
    TypeError,
  )
  await expect(
    api.send(conversationId, "😀".repeat(2048), {
      executionId: "😀".repeat(64),
      requestId: "😀".repeat(64),
    }),
  ).resolves.toMatchObject({ disposition: "queued" })
  expect(request).toHaveBeenCalledOnce()
})

it("enforces UTF-8 byte limits on server response identities", async () => {
  const request = vi.fn().mockResolvedValue({
    ...view,
    messages: [
      {
        executionId: "😀".repeat(65),
        userText: "hello",
        parts: [],
        status: "completed",
      },
    ],
  })
  const api = createConversationApi({ request }, () => "request")
  await expect(api.read(conversationId)).rejects.toThrow(
    "Invalid conversation executionId",
  )
})

it("reorders an immutable full queue and accepts each typed outcome", async () => {
  let finish!: (value: unknown) => void
  const request = vi.fn(
    () =>
      new Promise<unknown>((resolve) => {
        finish = resolve
      }),
  )
  const api = createConversationApi({ request }, () => "reorder-action")
  const order = ["second", "first"]
  const pending = api.reorder(conversationId, order)
  order.reverse()
  expect(request.mock.calls[0]).toEqual([
    "conversation.reorder",
    {
      conversationId: conversationId,
      requestId: "reorder-action",
      executionIds: ["second", "first"],
    },
  ])
  finish({ requestId: "reorder-action", outcome: "applied" })
  expect(await pending).toEqual({ requestId: "reorder-action", outcome: "applied" })
  for (const outcome of ["unchanged", "queue_changed", "priority_conflict"]) {
    const next = api.reorder(conversationId, [])
    finish({ requestId: "reorder-action", outcome })
    expect(await next).toEqual({ requestId: "reorder-action", outcome })
  }
})
it("rejects invalid queue orders locally and never retries uncertain reorder", async () => {
  const request = vi.fn().mockRejectedValue(new Error("ack lost"))
  const api = createConversationApi({ request }, () => "action")
  for (const order of [
    ["same", "same"],
    [""],
    Array.from({ length: 65 }, (_, i) => `e-${i}`),
  ]) {
    await expect(api.reorder(conversationId, order)).rejects.toBeInstanceOf(TypeError)
  }
  expect(request).not.toHaveBeenCalled()
  const error = await api
    .reorder(conversationId, ["first"])
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(NessaConversationControlError)
  expect(error).not.toHaveProperty("retry")
  expect(request).toHaveBeenCalledTimes(1)
})
it("rejects reorder acknowledgements with wrong action or unknown outcome", async () => {
  const request = vi.fn()
  const api = createConversationApi({ request }, () => "action")
  for (const receipt of [
    { requestId: "other", outcome: "applied" },
    { requestId: "action", outcome: "partial" },
  ]) {
    request.mockResolvedValueOnce(receipt)
    await expect(api.reorder(conversationId, [])).rejects.toBeInstanceOf(
      NessaConversationControlError,
    )
  }
})
