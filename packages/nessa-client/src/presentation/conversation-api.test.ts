import { NessaRpcError } from "../application/rpc-error.js"
import { agentOperationTimeoutMs } from "../application/agent-budgets.js"
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
  capabilities: {
    queue: true,
    steer: true,
    resume: true,
    permissions: true,
    imageInput: false,
    agentFeatures: {
      permissionDenial: "unknown",
      nativeHookSuppression: "unknown",
      compactionReporting: "unsupported_not_implemented",
      modelSwitchReporting: "unsupported_not_implemented",
      permissionDeferral: "unsupported_not_implemented",
      elicitationForwarding: "unknown",
      preToolPolicy: "unsupported_not_implemented",
      policyEndTurn: "unsupported_not_implemented",
      policyCloseSession: "unsupported_not_implemented",
      incomingElicitation: "unsupported_not_implemented",
    },
  },
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
  const error = await api
    .send(conversationId, "hello", [], [], options)
    .catch((error) => error)
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
    attachments: [],
    files: [],
  })
})
it("generates distinct identities per intentional message, retaining each on retry", async () => {
  let next = 0
  const request = vi.fn(async (_method: string, params: unknown) => ({
    executionId: (params as { executionId: string }).executionId,
    disposition: "queued",
  }))
  const api = createConversationApi({ request }, () => `id-${++next}`)
  expect(await api.send(conversationId, "same text", [])).toMatchObject({
    executionId: "id-1",
    requestId: "id-2",
  })
  expect(await api.send(conversationId, "same text", [])).toMatchObject({
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
    attachments: [],
    files: [],
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
      pending: [
        {
          executionId: "finished",
          text: "done",
          attachments: [],
          files: [],
          mode: "queued",
        },
      ],
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
          attachments: [],
          files: [],
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
    attachments: [],
    files: [],
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
        {
          executionId: "waiting",
          text: "one",
          attachments: [],
          files: [],
          mode: "queued",
        },
        {
          executionId: "waiting",
          text: "two",
          attachments: [],
          files: [],
          mode: "steering",
        },
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
          attachments: [],
          files: [],
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
          attachments: [],
          files: [],
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
          attachments: [],
          files: [],
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
        attachments: [],
        files: [],
        parts: [],
        status: "injected",
        steeringTarget: "omitted-target",
        steeringOffset: 2,
      },
      {
        executionId: "waiting",
        userText: "next",
        attachments: [],
        files: [],
        parts: [],
        status: "queued",
      },
    ],
    pending: [
      {
        executionId: "waiting",
        text: "next",
        attachments: [],
        files: [],
        mode: "queued",
      },
    ],
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
    api.steer(conversationId, "correction", [], [], { executionId: "expected" }),
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

it.each([
  // Three different situations, and only one of them is fixed by configuring
  // anything. A caller told the wrong one goes and changes what was never the
  // problem — so each states its own, and none of them is uncertain: the
  // gateway refused before the command reached an agent.
  ["conversations_not_configured", "not set up to run conversations"],
  ["agent_not_configured", "not set up for the agent"],
  ["agent_unsupported", "this version of Nessa cannot open"],
])(
  "reports %s as its own known rejection rather than unknown delivery",
  async (code, said) => {
    const request = vi.fn().mockRejectedValue(new NessaRpcError(code, code))
    const ids = [conversationId, "identity"]
    let next = 0
    const api = createConversationApi({ request }, () => ids[next++]!)
    const error = await api.create().catch((error) => error)
    expect(error).toBeInstanceOf(NessaConversationMutationError)
    expect(error.uncertain).toBe(false)
    expect(error.message).toContain(said)
  },
)

it.each(["toString", "constructor", "valueOf", "__proto__", "hasOwnProperty"])(
  "treats a gateway answering %s as a failure it cannot explain",
  async (code) => {
    // The code is wire text, and the refusal table is an object, so asking it
    // whether it holds a key answered for every name on `Object.prototype`.
    // A frame saying `toString` was read as a refusal this gateway had stated
    // — certain, so the panel would say the command was never admitted — with
    // a native function printed where the explanation belongs.
    const request = vi.fn().mockRejectedValue(new NessaRpcError(code, code))
    const ids = [conversationId, "identity"]
    let next = 0
    const api = createConversationApi({ request }, () => ids[next++]!)
    const error = await api.create().catch((error) => error)
    expect(error).toBeInstanceOf(NessaConversationMutationError)
    expect(error.uncertain).toBe(true)
    expect(error.message).toBe("Conversation command failed")
  },
)

it.each([
  // Every control resolves the conversation before it is dispatched, so each
  // meets exactly the refusals a creation meets. Saying "we do not know what
  // happened" for those is true of the delivery and useless to the person: the
  // reason is the whole fix, and the gateway had already given it.
  ["conversations_not_configured", "not set up to run conversations"],
  ["agent_not_configured", "not set up for the agent"],
  ["agent_unsupported", "this version of Nessa cannot open"],
])(
  "explains %s to a control, the way it explains it to a creation",
  async (code, said) => {
    const request = vi.fn().mockRejectedValue(new NessaRpcError(code, code))
    const api = createConversationApi({ request }, () => "identity")
    for (const failed of [
      await api.close(conversationId).catch((error) => error),
      await api
        .cancel(conversationId, "execution", "permission", "no longer needed")
        .catch((error) => error),
      await api.remove(conversationId, "execution").catch((error) => error),
      await api
        .answer(conversationId, "execution", "permission", "option")
        .catch((error) => error),
    ]) {
      expect(failed).toBeInstanceOf(NessaConversationControlError)
      expect(failed.message).toContain(said)
      expect(failed.uncertain).toBe(false)
    }
  },
)

it("names the remedy for a gateway missing the agent, not just the symptom", () => {
  // A person told only that the agent is not set up has nowhere to go. This is
  // the one refusal with an answer short enough to state, so it states it.
  const error = new NessaConversationMutationError(
    conversationId,
    "identity",
    undefined,
    new NessaRpcError("agent_not_configured", "agent_not_configured"),
    async () => undefined,
  )
  expect(error.message).toContain("agents.runtimes")
  expect(error.message).toContain("just server")
})
it("reports invalid requests as known pre-admission rejections", async () => {
  const request = vi
    .fn()
    .mockRejectedValue(new NessaRpcError("invalid_request", "invalid_request"))
  const api = createConversationApi({ request }, () => "identity")
  const sendError = await api.send(conversationId, "hello", []).catch((error) => error)
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
  expect(error.message).toContain("Retry normally succeeds")
})

it("treats a startup deadline on a control as a rejection before it was applied", async () => {
  const request = vi
    .fn()
    .mockRejectedValue(
      new NessaRpcError("agent_startup_deadline", "agent_startup_deadline"),
    )
  const api = createConversationApi({ request }, () => "identity")
  const error = await api.close(conversationId).catch((error) => error)
  expect(error).toBeInstanceOf(NessaConversationControlError)
  expect(error.code).toBe(ConversationErrorCode.AgentStartupDeadline)
  // Startup ends before the control could reach the provider, so nothing was
  // applied and the caller is not left guessing.
  expect(error.uncertain).toBe(false)
})

it("leaves an unrecognized gateway code untyped instead of guessing a meaning", async () => {
  const request = vi
    .fn()
    .mockRejectedValue(new NessaRpcError("invented_code", "invented_code"))
  const api = createConversationApi({ request }, () => "identity")
  const sendError = await api.send(conversationId, "hello").catch((error) => error)
  expect(sendError.code).toBeUndefined()
  expect(sendError.uncertain).toBe(true)
  expect(sendError.message).toBe("Conversation command failed")
  // Codes the socket answers with before dispatch are gateway rejections, but
  // they are not conversation codes, so they stay untyped here too.
  const forbidden = vi.fn().mockRejectedValue(new NessaRpcError("forbidden", "forbidden"))
  const closeError = await createConversationApi({ request: forbidden }, () => "identity")
    .close(conversationId)
    .catch((error) => error)
  expect(closeError.code).toBeUndefined()
})

it("enforces canonical conversation and UTF-8 byte limits before admission", async () => {
  const request = vi.fn().mockImplementation((_method, params) =>
    Promise.resolve({
      executionId: (params as { executionId: string }).executionId,
      disposition: "queued",
    }),
  )
  const api = createConversationApi({ request }, () => "request")
  expect(() => api.send("conversation", "hello", [])).toThrow(TypeError)
  expect(() => api.send(conversationId, "😀".repeat(2049), [])).toThrow(TypeError)
  expect(() =>
    api.send(conversationId, "hello", [], [], { executionId: "😀".repeat(65) }),
  ).toThrow(TypeError)
  expect(() =>
    api.send(conversationId, "hello", [], [], { requestId: "😀".repeat(65) }),
  ).toThrow(TypeError)
  await expect(
    api.send(conversationId, "😀".repeat(2048), [], [], {
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
        attachments: [],
        files: [],
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
    // Conversation commands can open an agent, so they raise the connection's
    // ordinary deadline rather than being abandoned mid-launch.
    { atLeastMs: agentOperationTimeoutMs },
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
it.each([
  ["a blank name", ""],
  ["a name over 32 bytes in ASCII", "x".repeat(33)],
  ["a name over 32 bytes in emoji", "\u{1f600}".repeat(9)],
])(
  "hands %s to the gateway to refuse, rather than throwing out of the call",
  async (_case, agent) => {
    // The agent is usually remembered rather than typed, so a bad one used to
    // throw a bare TypeError before `mutate` was entered and fail every command
    // of the launch with no reason and no retry. The gateway refuses a name it
    // does not run, and that refusal is what the caller gets.
    const request = vi
      .fn()
      .mockRejectedValueOnce(new NessaRpcError("agent_not_configured", "no runtime"))
      .mockResolvedValueOnce({ conversationId })
    const api = createConversationApi({ request }, () => "identity")
    const error = await api.create({ conversationId, agent }).catch((error) => error)
    expect(error).toBeInstanceOf(NessaConversationMutationError)
    expect(error.uncertain).toBe(false)
    expect(error.message).toContain("agents.runtimes")
    expect(request.mock.calls[0][1]).toEqual({
      conversationId,
      requestId: "identity",
      agent,
    })
    // The same command, unchanged, is what a retry sends.
    expect(await error.retry()).toEqual({ conversationId })
    expect(request.mock.calls[1]).toEqual(request.mock.calls[0])
  },
)
it("sends a usable agent name as the creation's own parameter", async () => {
  const request = vi.fn().mockResolvedValue({ conversationId })
  const api = createConversationApi({ request }, () => "identity")
  expect(await api.create({ conversationId, agent: "codex" })).toEqual({
    conversationId,
  })
  expect(request.mock.calls[0][1]).toEqual({
    conversationId,
    requestId: "identity",
    agent: "codex",
  })
  // Omitted stays omitted: the gateway's default is not a name the client invents.
  await api.create({ conversationId })
  expect(request.mock.calls[1][1]).toEqual({ conversationId, requestId: "identity" })
})

const image = {
  digest: `sha256:${"ab".repeat(32)}`,
  mimeType: "image/png",
  size: 3,
} as const
function queued() {
  return vi.fn(async (_method: string, params: unknown) => ({
    executionId: (params as { executionId: string }).executionId,
    disposition: "queued",
  }))
}
it("sends an image-only message, and steers with images the same way", async () => {
  const request = queued()
  const api = createConversationApi({ request }, () => "id")
  await api.send(conversationId, "", [image])
  await api.steer(conversationId, "  ", [image])
  const command = { conversationId, executionId: "id", requestId: "id", files: [] }
  const deadline = { atLeastMs: agentOperationTimeoutMs }
  expect(request.mock.calls).toEqual([
    ["conversation.send", { ...command, text: "", attachments: [image] }, deadline],
    ["conversation.steer", { ...command, text: "  ", attachments: [image] }, deadline],
  ])
})
it("sends a message that only points at files, and refuses a path it could not carry", () => {
  const request = queued()
  const api = createConversationApi({ request }, () => "id")
  const file = { path: "/Users/ada/report (final).pdf" }
  const command = { conversationId, executionId: "id", requestId: "id", attachments: [] }
  const deadline = { atLeastMs: agentOperationTimeoutMs }

  // A file alone is a whole message; the path travels and nothing else does.
  void api.send(conversationId, "", [], [file])
  expect(request.mock.calls).toEqual([
    ["conversation.send", { ...command, text: "", files: [file] }, deadline],
  ])

  // Refused here, before anything is sent, for every path the gateway's own
  // rule would refuse on arrival.
  for (const path of [
    "report.pdf",
    "~/report.pdf",
    "/Users/ada/a\nb.pdf",
    "/Users/ada/a\u0000b.pdf",
    `/${"a".repeat(4096)}`,
    // Parts that would not survive being written as a URI: dropped, or
    // resolved away, either way naming a path nobody wrote.
    "//Users/ada/report.pdf",
    "/Users//ada/report.pdf",
    "/Users/ada/",
    "/Users/ada/..",
    "/Users/../etc/passwd",
    "/Users/./ada/report.pdf",
    // C1 controls, and the delete character. The gateway refuses these with
    // `char::is_control`; the client used to accept them and leave it to answer
    // `invalid_request`, whose panel sentence is deliberately empty on the
    // grounds that the client already spoke — so nothing was said at all.
    "/Users/ada/rep\u0085rt.pdf",
    "/Users/ada/rep\u0090rt.pdf",
    "/Users/ada/rep\u009frt.pdf",
    "/Users/ada/rep\u007frt.pdf",
  ])
    expect(() => api.send(conversationId, "look", [], [{ path }])).toThrow(TypeError)
  // And the ordinary names next to them still go: a leading dot, a name of
  // three dots, a colon, and a percent sign.
  for (const path of [
    "/Users/ada/.zshrc",
    "/Users/ada/...",
    "/Users/ada/2026-09-20 10:30.txt",
    "/Users/ada/100% done.pdf",
  ])
    expect(() => api.send(conversationId, "look", [], [{ path }])).not.toThrow()
  expect(() =>
    api.send(conversationId, "look", [], [{ path: "/a.pdf", name: "a.pdf" } as never]),
  ).toThrow(TypeError)
  expect(() =>
    api.send(
      conversationId,
      "look",
      [],
      Array.from({ length: 11 }, (_, index) => ({ path: `/${index}.pdf` })),
    ),
  ).toThrow(TypeError)
  expect(request.mock.calls).toHaveLength(5)
})
it("retries with the images it was given, whatever the caller did to its list since", async () => {
  const request = vi
    .fn()
    .mockRejectedValueOnce(new Error("connection lost"))
    .mockResolvedValueOnce({ executionId: "execution", disposition: "queued" })
  const api = createConversationApi({ request }, () => "unused")
  const attachments: { digest: string; mimeType: "image/png"; size: number }[] = [
    { ...image },
  ]
  const error = await api
    .send(conversationId, "look", attachments, [], { executionId: "execution" })
    .catch((error) => error)
  attachments[0]!.size = 4
  attachments.push({ ...image })
  await error.retry()
  expect(request.mock.calls[1]).toEqual(request.mock.calls[0])
  expect(request.mock.calls[1][1].attachments).toEqual([image])
})
it("refuses a message the gateway would refuse, before admission", async () => {
  const request = queued()
  const api = createConversationApi({ request }, () => "id")
  const refused: [string, unknown][] = [
    ["", []],
    ["   ", []],
    ["😀".repeat(2049), [image]],
    ["hello", [{ ...image, digest: image.digest.toUpperCase() }]],
    ["hello", [{ ...image, mimeType: "image/svg+xml" }]],
    ["hello", [{ ...image, size: 0 }]],
    // `ImageAttachment.size` maximum in the schema is 5242880.
    ["hello", [{ ...image, size: 5_242_881 }]],
    ["hello", [{ ...image, bytes: "AAAA" }]],
    ["hello", Array.from({ length: 11 }, () => image)],
    ["hello", Array.from({ length: 3 }, () => ({ ...image, size: 4 * 1024 * 1024 }))],
    // Omitted is a message of text alone, so the text still has to say something.
    ["", undefined],
    ["hello", "not a list"],
  ]
  for (const [text, attachments] of refused)
    expect(() => api.send(conversationId, text, attachments as never)).toThrow(TypeError)
  expect(request).not.toHaveBeenCalled()
  await api.send(
    conversationId,
    "hello",
    Array.from({ length: 10 }, () => ({ ...image, size: 1024 * 1024 })),
  )
  // And a message of text alone needs no list at all.
  await api.send(conversationId, "hello")
  expect(request).toHaveBeenCalledTimes(2)
})

it("accepts an image of exactly the schema's maximum size", async () => {
  const request = queued()
  await createConversationApi({ request }, () => "id").send(conversationId, "", [
    { ...image, size: 5_242_880 },
  ])
  expect(request).toHaveBeenCalledOnce()
})

it.each([
  "invalid_request",
  "agent_not_configured",
  "image_input_unsupported",
  "attachment_not_found",
  "attachment_unavailable",
  "conversation_not_found",
  "conversation_capacity",
  // The gateway cannot read this conversation's saved state and caches that,
  // so it answers without opening an agent at all. It had been left out of the
  // list this used to be, which reported it as uncertain and made two
  // sentences the panel had written for it unreachable; the list is a total
  // `switch` now, so the next code has to be answered rather than forgotten.
  "conversation_state_unreadable",
] as const)(
  "knows %s was decided before admission, for send and steer alike",
  async (code) => {
    // The message names a different code: only the typed code is read.
    const request = vi
      .fn()
      .mockRejectedValue(new NessaRpcError(code, "temporarily_unavailable"))
    const api = createConversationApi({ request }, () => "id")
    for (const submit of [api.send, api.steer]) {
      const error = await submit(conversationId, "look", [image]).catch((error) => error)
      expect(error).toBeInstanceOf(NessaConversationMutationError)
      expect(error).toMatchObject({ uncertain: false, code })
    }
  },
)

it.each([
  // Also what a supervising task reports after losing work it had admitted.
  "temporarily_unavailable",
  "conversation_storage_unavailable",
  "audit_unavailable",
  "agent_operation_failed",
  "submission_conflict",
  "a_code_nobody_taught_this_client",
])("leaves %s uncertain: nothing proves the message was not admitted", async (code) => {
  const request = vi
    .fn()
    .mockRejectedValue(new NessaRpcError(code, "attachment_not_found"))
  const error = await createConversationApi({ request }, () => "id")
    .send(conversationId, "look", [image])
    .catch((error) => error)
  // The code is still reported when this client knows it; what it does not
  // report is that the message was refused.
  expect(error).toMatchObject({ uncertain: true })
})

it("does not read a rejection out of an error that is not the gateway's answer", async () => {
  const request = vi.fn().mockRejectedValue(new Error("attachment_not_found"))
  const error = await createConversationApi({ request }, () => "id")
    .send(conversationId, "look", [image])
    .catch((error) => error)
  expect(error).toMatchObject({ uncertain: true, code: undefined })
})
