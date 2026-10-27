import { questionTargets } from "../application/question-target.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { expect, it, vi } from "vitest"
import { executionTargets } from "../application/execution-target.js"
import { permissionTargets } from "../application/permission-target.js"
import { conversationTitle, createConversationApi } from "./conversation-api.js"
import { conversationDeleteTimeoutMs } from "../application/agent-budgets.js"
import {
  NessaConversationMutationError,
  NessaConversationControlError,
} from "../application/conversation-mutation-error.js"
import {
  bounds,
  ConversationErrorCode,
  type ConversationToolPolicy,
  type ConversationPermission,
  type ConversationView,
} from "../generated/product.js"

const conversationId = "00000000-0000-4000-8000-000000000001"

const view: ConversationView = {
  conversationId,
  artifacts: [],
  notes: [],
  unreadable: [],
  unreadableTruncated: false,
  handoffs: [],
  approvalMode: "ask",
  model: "claude-sonnet-5",
  approvalModes: [
    {
      id: "ask",
      name: "Provider asks",
      description: "The provider requests approval where required.",
    },
  ],
  title: "Flights to Lisbon",
  revision: "opaque-1",
  serverNowMs: 1_700_000_000_000,
  messages: [],
  pending: [],
  permissions: [],
  questions: [],
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
      nativeHooks: "off",
      compactionReporting: "unsupported_not_implemented",
      modelSwitchReporting: "unsupported_not_implemented",
      permissionDeferral: "unsupported_not_implemented",
      elicitationForwarding: "unknown",
      preToolPolicy: "unsupported_not_implemented",
      policyEndTurn: "unsupported",
      policyCloseSession: "unsupported",
      incomingElicitation: "unsupported",
    },
  },
  lifecycle: { phase: "attached" },
  truncated: false,
  queueComplete: true,
  transcriptState: "complete",
}
function reviewTarget(permissionId = "permission") {
  return permissionTargets({
    conversationId,
    permissions: [
      {
        review: "0123456789abcdef0123456789abcdef",
        executionId: "execution",
        permissionId,
        options: [{ id: "allow" }, { id: "option" }],
      },
    ],
  })[0]!
}

it("passes the initial model and mode only on creation", async () => {
  const request = vi.fn().mockResolvedValue({ conversationId })
  const api = createConversationApi({ request }, () => "generated")
  await api.create({
    conversationId,
    requestId: "create-model",
    agent: "claude",
    model: "claude-sonnet-5",
    approvalMode: "auto",
  })
  expect(request).toHaveBeenCalledWith("conversation.create", {
    conversationId,
    requestId: "create-model",
    agent: "claude",
    model: "claude-sonnet-5",
    approvalMode: "auto",
  })
})

it("passes the SSH host a conversation runs on only when one is named", async () => {
  const request = vi.fn().mockResolvedValue({ conversationId })
  const api = createConversationApi({ request }, () => "generated")
  await api.create({ conversationId, requestId: "create-here" })
  await api.create({ conversationId, requestId: "create-there", environment: "devbox" })
  expect(request.mock.calls.map(([, params]) => params)).toEqual([
    { conversationId, requestId: "create-here" },
    { conversationId, requestId: "create-there", environment: "devbox" },
  ])
})

it("passes the channel a conversation is in only when one is named", async () => {
  const request = vi.fn().mockResolvedValue({ conversationId })
  const api = createConversationApi({ request }, () => "generated")
  await api.create({ conversationId, requestId: "create-default" })
  await api.create({ conversationId, requestId: "create-in", channelId: "ch_aaaaaaaaaa" })
  expect(request.mock.calls.map(([, params]) => params)).toEqual([
    { conversationId, requestId: "create-default" },
    { conversationId, requestId: "create-in", channelId: "ch_aaaaaaaaaa" },
  ])
})

it("correlates an acknowledged mode change and rejects a mismatched reply", async () => {
  const request = vi.fn().mockResolvedValue({ requestId: "mode-1", mode: "auto" })
  const api = createConversationApi({ request }, () => "generated")
  await expect(
    api.setApprovalMode(conversationId, "auto", { requestId: "mode-1" }),
  ).resolves.toEqual({
    requestId: "mode-1",
    mode: "auto",
  })
  expect(request).toHaveBeenCalledWith("conversation.setApprovalMode", {
    conversationId,
    requestId: "mode-1",
    mode: "auto",
  })
  request.mockResolvedValue({ requestId: "mode-1", mode: "ask" })
  await expect(
    api.setApprovalMode(conversationId, "auto", { requestId: "mode-1" }),
  ).rejects.toBeInstanceOf(NessaConversationControlError)
})

it("sets the gateway usage-limit preference without naming a conversation", async () => {
  const request = vi.fn().mockResolvedValue({ requestId: "pref-1", enabled: true })
  const api = createConversationApi({ request }, () => "id")
  await expect(
    api.setUsageLimitAutoContinue(true, { requestId: "pref-1" }),
  ).resolves.toEqual({
    requestId: "pref-1",
    enabled: true,
  })
  expect(request).toHaveBeenCalledWith("conversation.setUsageLimitAutoContinue", {
    requestId: "pref-1",
    enabled: true,
  })
  request.mockResolvedValue({ requestId: "pref-1", enabled: false })
  await expect(
    api.setUsageLimitAutoContinue(true, { requestId: "pref-1" }),
  ).rejects.toThrow(/contradicts/)
})
it("validates attachment lifecycle fields and their cross-field agreement", async () => {
  const api = createConversationApi(
    { request: vi.fn().mockResolvedValue(view) },
    () => "id",
  )
  await expect(api.read(conversationId)).resolves.toMatchObject({
    lifecycle: { phase: "attached" },
  })
  for (const lifecycle of [
    { phase: "failed" },
    { phase: "starting", failure: { code: "provider", message: "failed" } },
    { phase: "failed", failure: { code: "unknown", message: "failed" } },
    { phase: "failed", failure: { code: "provider", message: "" } },
    {
      phase: "attached",
      evidenceFailure: { code: "provider", message: "failed" },
    },
  ]) {
    const invalid = createConversationApi(
      { request: vi.fn().mockResolvedValue({ ...view, lifecycle }) },
      () => "id",
    )
    await expect(invalid.read(conversationId)).rejects.toThrow()
  }
  const exactUtf8 = createConversationApi(
    {
      request: vi.fn().mockResolvedValue({
        ...view,
        lifecycle: {
          phase: "failed",
          failure: { code: "provider", message: "😀".repeat(512) },
        },
      }),
    },
    () => "id",
  )
  await expect(exactUtf8.read(conversationId)).resolves.toBeDefined()
  const exactEvidenceUtf8 = createConversationApi(
    {
      request: vi.fn().mockResolvedValue({
        ...view,
        lifecycle: {
          phase: "attached",
          evidenceFailure: { code: "audit", message: "😀".repeat(512) },
        },
      }),
    },
    () => "id",
  )
  await expect(exactEvidenceUtf8.read(conversationId)).resolves.toBeDefined()
  const oversizedUtf8 = createConversationApi(
    {
      request: vi.fn().mockResolvedValue({
        ...view,
        lifecycle: {
          phase: "failed",
          failure: { code: "provider", message: "😀".repeat(513) },
        },
      }),
    },
    () => "id",
  )
  await expect(oversizedUtf8.read(conversationId)).rejects.toThrow(
    "Invalid conversation message",
  )
  const oversizedEvidenceUtf8 = createConversationApi(
    {
      request: vi.fn().mockResolvedValue({
        ...view,
        lifecycle: {
          phase: "absent",
          evidenceFailure: { code: "audit", message: "😀".repeat(513) },
        },
      }),
    },
    () => "id",
  )
  await expect(oversizedEvidenceUtf8.read(conversationId)).rejects.toThrow(
    "Invalid conversation message",
  )
})

it("uses the ordinary configured request deadline for create, read, and send", async () => {
  const request = vi.fn(async (method: string, params: unknown) => {
    if (method === "conversation.create") return { conversationId }
    if (method === "conversation.read") return view
    return {
      executionId: (params as { executionId: string }).executionId,
      disposition: "queued",
    }
  })
  const api = createConversationApi({ request }, () => "identity")
  await api.create({ conversationId })
  await api.read(conversationId)
  await api.submit(api.prepare(conversationId, "hello"))
  expect(request.mock.calls.map((call) => call.length)).toEqual([2, 2, 2])
})
it("owns immutable message and action identities through an uncertain retry", async () => {
  const request = vi
    .fn()
    .mockRejectedValueOnce(new Error("connection lost"))
    .mockResolvedValueOnce({ executionId: "execution", disposition: "queued" })
  const api = createConversationApi({ request }, () => "unused")
  const options = { executionId: "execution", requestId: "action" }
  const error = await api
    .submit(api.prepare(conversationId, "hello", [], [], options))
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
  expect(await error.retry()).toMatchObject({
    conversationId,
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
  expect(await api.submit(api.prepare(conversationId, "same text", []))).toMatchObject({
    executionId: "id-1",
    requestId: "id-2",
  })
  expect(await api.submit(api.prepare(conversationId, "same text", []))).toMatchObject({
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
  await api.remove(
    executionTargets({ ...view, pending: [{ executionId: "execution" }] })[0]!,
  )
  await api.answer(reviewTarget("review"), "allow")
  await api.cancel(reviewTarget("review"), "user withdrew")
  await api.close(conversationId)
  await api.archive(conversationId)
  await api.unarchive(conversationId)
  await api.delete(conversationId)
  expect(request.mock.calls.map((call) => call[0])).toEqual([
    "conversation.remove",
    "conversation.answer",
    "conversation.cancel",
    "conversation.close",
    "conversation.archive",
    "conversation.unarchive",
    "conversation.delete",
  ])
  expect(request.mock.calls.at(-1)![1]).toEqual({ conversationId, requestId: "action" })
  expect(request.mock.calls[1][1]).toEqual({
    conversationId: conversationId,
    requestId: "action",
    executionId: "execution",
    review: "0123456789abcdef0123456789abcdef",
    permissionId: "review",
    optionId: "allow",
  })
})
it("makes a typed title one the gateway keeps", () => {
  expect(conversationTitle("  Rain \n  plan\t ")).toBe("Rain plan")
  expect(conversationTitle(" \n\t ")).toBeNull()
  // Control characters, and U+0085, which JavaScript's \s leaves and the gateway folds.
  expect(conversationTitle("Plan\u0007trip\u0085now")).toBe("Plan trip now")
  expect(conversationTitle("\u0000\u001f")).toBeNull()
  // Cut by code point, at the published maximum, and never ending in a space.
  const long = conversationTitle(`${"🌧".repeat(48)} more`)
  expect(Array.from(long ?? "")).toHaveLength(48)
  expect(conversationTitle(`${"a".repeat(47)} b`)).toBe("a".repeat(47))
})

it("renames a conversation with the title as given and its own request identity", async () => {
  const request = vi.fn(async (_method: string, params: unknown) => ({
    requestId: (params as { requestId: string }).requestId,
    applied: true,
  }))
  const api = createConversationApi({ request }, () => "rename")
  await expect(api.rename(conversationId, "Lisbon in May")).resolves.toEqual({
    requestId: "rename",
    applied: true,
  })
  expect(request).toHaveBeenCalledWith("conversation.rename", {
    conversationId,
    requestId: "rename",
    title: "Lisbon in May",
  })
  // The gateway decides what a title may be: the client sends it unaltered.
  await api.rename(conversationId, " two  spaces ", { requestId: "again" })
  expect(request.mock.calls.at(-1)![1]).toEqual({
    conversationId,
    requestId: "again",
    title: " two  spaces ",
  })
  // Refused before anything is sent, as every control's target is.
  expect(() => api.rename("not-a-uuid", "Lisbon")).toThrow(TypeError)
  // An answer for another request is not this rename's.
  request.mockResolvedValueOnce({ requestId: "other", applied: true })
  await expect(api.rename(conversationId, "Lisbon")).rejects.toThrow()
})
it("switches a conversation's agent under its own request identity", async () => {
  const request = vi.fn(async (_method: string, params: unknown) => ({
    requestId: (params as { requestId: string }).requestId,
    applied: true,
  }))
  const api = createConversationApi({ request }, () => "switch")
  await expect(
    api.switchAgent(conversationId, "codex", "gpt-5.5-codex"),
  ).resolves.toEqual({ requestId: "switch", applied: true })
  expect(request).toHaveBeenCalledWith("conversation.switchAgent", {
    conversationId,
    requestId: "switch",
    agent: "codex",
    model: "gpt-5.5-codex",
  })
  expect(() => api.switchAgent("not-a-uuid", "codex", "m")).toThrow(TypeError)
  request.mockResolvedValueOnce({ requestId: "other", applied: true })
  await expect(api.switchAgent(conversationId, "codex", "m")).rejects.toThrow()
})
it("forks a conversation into the identity it was given", async () => {
  const forked = "6f1c1c4e-2b0a-4f0e-9a51-0d7f7b0f3a11"
  const request = vi.fn(async (_method: string, params: unknown) => ({
    conversationId: (params as { conversationId: string }).conversationId,
  }))
  const api = createConversationApi({ request }, () => forked)
  await expect(api.fork(conversationId)).resolves.toEqual({ conversationId: forked })
  expect(request).toHaveBeenCalledWith("conversation.fork", {
    conversationId: forked,
    requestId: forked,
    sourceConversationId: conversationId,
  })
  expect(() => api.fork("not-a-uuid")).toThrow(TypeError)
  // An answer naming another conversation is not this fork's.
  request.mockResolvedValueOnce({ conversationId })
  await expect(api.fork(conversationId)).rejects.toThrow()
})
it("stops the named turn without closing the attachment", async () => {
  const request = vi.fn(async () => ({
    requestId: "action",
    stage: "settled",
    outcome: "cancelled",
  }))
  const api = createConversationApi({ request }, () => "action")
  await expect(
    api.stop(executionTargets({ ...view, pending: [{ executionId: "turn" }] })[0]!),
  ).resolves.toEqual({
    requestId: "action",
    stage: "settled",
    outcome: "cancelled",
  })
  expect(request).toHaveBeenCalledWith("conversation.stop", {
    conversationId,
    requestId: "action",
    executionId: "turn",
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
            { offset: 0, kind: "thought", text: "", toolId: "", noticeId: "" },
            { offset: 1, kind: "text", text: "", toolId: "", noticeId: "" },
          ],
          status: "maybe",
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
    review: "0123456789abcdef0123456789abcdef",
    origin: "harness",
    toolName: "shell",
  }
  const tool = {
    executionId: "first",
    toolId: "tool",
    title: "Run",
    kind: "execute",
    status: "completed",
    input: "{}",
    details: "done",
    nativeHooks: "off",
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
    transcriptState: "complete",
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
        review: "0123456789abcdef0123456789abcdef",
        origin: "harness",
        toolName: "shell",
      },
    ],
    tools: [
      {
        executionId: "omitted-execution",
        toolId: "omitted-tool",
        title: "Run",
        kind: "execute",
        status: "running",
        input: "{}",
        details: "",
        nativeHooks: "off",
      },
    ],
  })
  const api = createConversationApi({ request }, () => "id")
  await expect(api.read(conversationId)).resolves.toMatchObject({
    truncated: true,
    queueComplete: false,
    transcriptState: "complete",
  })
})
it("rejects a response acknowledging a different execution or action", async () => {
  const request = vi
    .fn()
    .mockResolvedValue({ executionId: "other", disposition: "queued" })
  const api = createConversationApi({ request }, () => "id")
  await expect(
    api.submit(
      api.prepare(conversationId, "correction", [], [], {
        ...{ executionId: "expected" },
        mode: "steering",
      }),
    ),
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
    () =>
      api.remove(
        executionTargets({ ...view, pending: [{ executionId: "execution" }] })[0]!,
      ),
    () => api.answer(reviewTarget(), "allow"),
    () => api.cancel(reviewTarget(), "withdrawn"),
  ]
  for (const control of controls) {
    const error = await control().catch((error: unknown) => error)
    expect(error).toBeInstanceOf(NessaConversationControlError)
    expect(error).toMatchObject({ uncertain: true, requestId: "action" })
    expect(error).not.toHaveProperty("retry")
  }
  expect(request).toHaveBeenCalledTimes(4)
})

it("correlates a question answer acknowledgement with its action", async () => {
  const request = vi
    .fn()
    .mockResolvedValue({ requestId: "question-action", applied: true })
  const api = createConversationApi({ request }, () => "question-action")
  await expect(
    api.answerQuestion(
      questionTargets({
        conversationId,
        questions: [
          {
            question: "0123456789abcdef0123456789abcdef",
            executionId: "execution",
            questionId: "ask",
            message: "Ask",
            questions: [],
          },
        ],
      })[0]!,
      null,
    ),
  ).resolves.toEqual({ requestId: "question-action", applied: true })
  expect(request).toHaveBeenCalledWith("conversation.answerQuestion", {
    conversationId,
    executionId: "execution",
    question: "0123456789abcdef0123456789abcdef",
    questionId: "ask",
    requestId: "question-action",
    choices: null,
  })
})

it.each([false, true])(
  "reconciles a lost question answer without replay when consumed is %s",
  async (consumed) => {
    const ask = {
      question: "0123456789abcdef0123456789abcdef",
      executionId: "execution",
      questionId: "ask",
      message: "Choose an environment",
      questions: [
        {
          key: "environment",
          prompt: "Where?",
          multiSelect: false,
          freeText: false,
          required: true,
          options: [{ value: "staging", label: "Staging" }],
        },
      ],
    }
    let waiting = true
    let answers = 0
    const request = vi.fn(async (method: string, params: Record<string, unknown>) => {
      if (method === "conversation.read")
        return {
          ...view,
          messages: [
            {
              executionId: "execution",
              userText: "Deploy",
              attachments: [],
              files: [],
              parts: [],
              status: "running",
            },
          ],
          // A view lists it by its entry; what it asks is read apart.
          questions: waiting ? [{ question: ask.question, message: ask.message }] : [],
        }
      expect(method).toBe("conversation.answerQuestion")
      expect(waiting).toBe(true)
      answers += 1
      if (answers === 1) {
        if (consumed) waiting = false
        // The client cannot distinguish loss before consumption from loss of
        // an acknowledgement after consumption. The current read can.
        throw new Error("acknowledgement lost")
      }
      waiting = false
      return { requestId: params.requestId, applied: true }
    })
    const api = createConversationApi({ request }, () => "generated")
    const choices = [{ key: "environment", values: ["staging"] }]
    const error = await api
      .answerQuestion(
        questionTargets({ conversationId, questions: [ask] })[0]!,
        choices,
        {
          requestId: "lost-answer",
        },
      )
      .catch((error: unknown) => error)
    expect(error).toBeInstanceOf(NessaConversationControlError)
    expect(error).toMatchObject({
      uncertain: true,
      conversationId,
      executionId: "execution",
      requestId: "lost-answer",
      permissionSelection: undefined,
    })
    expect(error).not.toHaveProperty("retry")
    const current = await api.read(conversationId)
    expect(current.questions).toEqual(
      consumed ? [] : [{ question: ask.question, message: ask.message }],
    )
    expect(answers).toBe(1)
    if (current.questions.length) {
      // A fresh read may justify a new deliberate action, not replay of the
      // failed action. The provider remains the owner of ask consumption.
      await expect(
        api.answerQuestion(
          questionTargets({ conversationId, questions: [ask] })[0]!,
          choices,
          {
            requestId: "deliberate-answer",
          },
        ),
      ).resolves.toEqual({ requestId: "deliberate-answer", applied: true })
      expect(answers).toBe(2)
      expect(request.mock.calls.at(-1)?.[1].requestId).toBe("deliberate-answer")
    }
  },
)

it("keeps question refusal and mismatched acknowledgement in the shared control contract", async () => {
  const request = vi
    .fn()
    .mockRejectedValueOnce(new NessaRpcError("invalid_request", "invalid_request"))
    .mockResolvedValueOnce({ requestId: "another-action", applied: true })
  const api = createConversationApi({ request }, () => "question-action")
  const refused = await api
    .answerQuestion(
      questionTargets({
        conversationId,
        questions: [
          {
            question: "0123456789abcdef0123456789abcdef",
            executionId: "execution",
            questionId: "ask",
            message: "Ask",
            questions: [],
          },
        ],
      })[0]!,
      null,
    )
    .catch((error: unknown) => error)
  expect(refused).toBeInstanceOf(NessaConversationControlError)
  expect(refused).toMatchObject({ uncertain: false, requestId: "question-action" })
  expect(refused).not.toHaveProperty("retry")
  const mismatched = await api
    .answerQuestion(
      questionTargets({
        conversationId,
        questions: [
          {
            question: "0123456789abcdef0123456789abcdef",
            executionId: "execution",
            questionId: "ask",
            message: "Ask",
            questions: [],
          },
        ],
      })[0]!,
      null,
    )
    .catch((error: unknown) => error)
  expect(mismatched).toBeInstanceOf(NessaConversationControlError)
  expect(mismatched).toMatchObject({ uncertain: true, requestId: "question-action" })
  expect(mismatched).not.toHaveProperty("retry")
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
      .answer(reviewTarget(), "allow")
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
      .answer(reviewTarget(), "allow")
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
    .remove(executionTargets({ ...view, pending: [{ executionId: "execution" }] })[0]!)
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
      await api.cancel(reviewTarget(), "no longer needed").catch((error) => error),
      await api
        .remove(
          executionTargets({ ...view, pending: [{ executionId: "execution" }] })[0]!,
        )
        .catch((error) => error),
      await api.answer(reviewTarget(), "option").catch((error) => error),
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
  const sendError = await api
    .submit(api.prepare(conversationId, "hello", []))
    .catch((error) => error)
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
  const error = await api
    .submit(api.prepare(conversationId, "hello"))
    .catch((error) => error)
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
  const sendError = await api
    .submit(api.prepare(conversationId, "hello"))
    .catch((error) => error)
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
  expect(() => api.submit(api.prepare("conversation", "hello", []))).toThrow(TypeError)
  expect(() => api.submit(api.prepare(conversationId, "😀".repeat(2049), []))).toThrow(
    TypeError,
  )
  expect(() =>
    api.submit(
      api.prepare(conversationId, "hello", [], [], { executionId: "😀".repeat(65) }),
    ),
  ).toThrow(TypeError)
  expect(() =>
    api.submit(
      api.prepare(conversationId, "hello", [], [], { requestId: "😀".repeat(65) }),
    ),
  ).toThrow(TypeError)
  await expect(
    api.submit(
      api.prepare(conversationId, "😀".repeat(2048), [], [], {
        executionId: "😀".repeat(64),
        requestId: "😀".repeat(64),
      }),
    ),
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
  await api.submit(api.prepare(conversationId, "", [image]))
  await api.submit(api.prepare(conversationId, "  ", [image], [], { mode: "steering" }))
  const command = { conversationId, executionId: "id", requestId: "id", files: [] }
  expect(request.mock.calls).toEqual([
    ["conversation.send", { ...command, text: "", attachments: [image] }],
    ["conversation.steer", { ...command, text: "  ", attachments: [image] }],
  ])
})
it("sends a message that only points at files, and refuses a path it could not carry", () => {
  const request = queued()
  const api = createConversationApi({ request }, () => "id")
  const file = { path: "/Users/ada/report (final).pdf" }
  const command = { conversationId, executionId: "id", requestId: "id", attachments: [] }

  // A file alone is a whole message; the path travels and nothing else does.
  void api.submit(api.prepare(conversationId, "", [], [file]))
  expect(request.mock.calls).toEqual([
    ["conversation.send", { ...command, text: "", files: [file] }],
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
    expect(() => api.submit(api.prepare(conversationId, "look", [], [{ path }]))).toThrow(
      TypeError,
    )
  // And the ordinary names next to them still go: a leading dot, a name of
  // three dots, a colon, and a percent sign.
  for (const path of [
    "/Users/ada/.zshrc",
    "/Users/ada/...",
    "/Users/ada/2026-09-20 10:30.txt",
    "/Users/ada/100% done.pdf",
  ])
    expect(() =>
      api.submit(api.prepare(conversationId, "look", [], [{ path }])),
    ).not.toThrow()
  expect(() =>
    api.submit(
      api.prepare(
        conversationId,
        "look",
        [],
        [{ path: "/a.pdf", name: "a.pdf" } as never],
      ),
    ),
  ).toThrow(TypeError)
  expect(() =>
    api.submit(
      api.prepare(
        conversationId,
        "look",
        [],
        Array.from({ length: 11 }, (_, index) => ({ path: `/${index}.pdf` })),
      ),
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
    .submit(
      api.prepare(conversationId, "look", attachments, [], { executionId: "execution" }),
    )
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
    expect(() =>
      api.submit(api.prepare(conversationId, text, attachments as never)),
    ).toThrow(TypeError)
  expect(request).not.toHaveBeenCalled()
  await api.submit(
    api.prepare(
      conversationId,
      "hello",
      Array.from({ length: 10 }, () => ({ ...image, size: 1024 * 1024 })),
    ),
  )
  // And a message of text alone needs no list at all.
  await api.submit(api.prepare(conversationId, "hello"))
  expect(request).toHaveBeenCalledTimes(2)
})

it("accepts an image of exactly the schema's maximum size", async () => {
  const request = queued()
  await createConversationApi({ request }, () => "id").submit(
    createConversationApi({ request }, () => "id").prepare(conversationId, "", [
      { ...image, size: 5_242_880 },
    ]),
  )
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
    for (const mode of ["queued", "steering"] as const) {
      const error = await api
        .submit(api.prepare(conversationId, "look", [image], [], { mode }))
        .catch((error) => error)
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
    .submit(
      createConversationApi({ request }, () => "id").prepare(conversationId, "look", [
        image,
      ]),
    )
    .catch((error) => error)
  // The code is still reported when this client knows it; what it does not
  // report is that the message was refused.
  expect(error).toMatchObject({ uncertain: true })
})

it("does not read a rejection out of an error that is not the gateway's answer", async () => {
  const request = vi.fn().mockRejectedValue(new Error("attachment_not_found"))
  const error = await createConversationApi({ request }, () => "id")
    .submit(
      createConversationApi({ request }, () => "id").prepare(conversationId, "look", [
        image,
      ]),
    )
    .catch((error) => error)
  expect(error).toMatchObject({ uncertain: true, code: undefined })
})

const summary = {
  conversationId,
  title: "Flights to Lisbon",
  preview: "Done. Confirmation K7Q2PX is in your inbox.",
  createdAtMs: 1790200000000,
  updatedAtMs: 1790200300000,
  running: false,
  archived: false,
}

it("lists conversations with an empty request and keeps what the gateway has not got as null", async () => {
  const request = vi.fn().mockResolvedValue({
    conversations: [
      summary,
      {
        ...summary,
        conversationId: "00000000-0000-4000-8000-000000000002",
        title: null,
        preview: null,
        running: true,
        archived: false,
      },
    ],
    complete: false,
  })
  const api = createConversationApi({ request }, () => "id")
  const listed = await api.list()
  // Whether the bound cut the list is the gateway's to say, and is kept.
  expect(listed.complete).toBe(false)
  expect(request).toHaveBeenCalledWith("conversation.list", {})
  // Asking for archived conversations and being answered with others is a list
  // that contradicts its own request.
  await expect(api.list({ archived: true })).rejects.toThrow(/archived filter/)
  expect(request).toHaveBeenLastCalledWith("conversation.list", { archived: true })
  expect(
    listed.conversations.map((row) => [row.title, row.preview, row.running]),
  ).toEqual([
    ["Flights to Lisbon", "Done. Confirmation K7Q2PX is in your inbox.", false],
    [null, null, true],
  ])
})

it("refuses a list row outside the schema, or one conversation listed twice", async () => {
  const invalidRows = [
    { ...summary, title: "" },
    { ...summary, preview: "x".repeat(513) },
    { ...summary, conversationId: "not-a-uuid" },
    { ...summary, updatedAtMs: -1 },
    { ...summary, running: "no" },
    { ...summary, archived: undefined },
    { ...summary, unread: 2 },
  ]
  for (const row of invalidRows) {
    const api = createConversationApi(
      { request: vi.fn().mockResolvedValue({ conversations: [row], complete: true }) },
      () => "id",
    )
    await expect(api.list()).rejects.toThrow(/conversation/i)
  }
  const twice = createConversationApi(
    {
      request: vi
        .fn()
        .mockResolvedValue({ conversations: [summary, summary], complete: true }),
    },
    () => "id",
  )
  await expect(twice.list()).rejects.toThrow(/conversationId/)
  const overBound = createConversationApi(
    {
      request: vi.fn().mockResolvedValue({
        conversations: Array.from({ length: 501 }, (_, index) => ({
          ...summary,
          conversationId: `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
        })),
        complete: false,
      }),
    },
    () => "id",
  )
  await expect(overBound.list()).rejects.toThrow(/conversations/)
  // A list that does not say whether it is whole cannot be read as either.
  for (const complete of [undefined, "yes"]) {
    const unsaid = createConversationApi(
      { request: vi.fn().mockResolvedValue({ conversations: [summary], complete }) },
      () => "id",
    )
    await expect(unsaid.list()).rejects.toThrow(/complete/)
  }
})

it("keeps the channel a listed conversation is in, and refuses one the schema cannot name", async () => {
  const inChannel = { ...summary, channelId: "ch_abcdefgh27" }
  const api = createConversationApi(
    {
      request: vi.fn().mockResolvedValue({ conversations: [inChannel], complete: true }),
    },
    () => "id",
  )
  expect((await api.list()).conversations).toEqual([inChannel])
  for (const channelId of ["default", "ch_bad", 7]) {
    const refused = createConversationApi(
      {
        request: vi.fn().mockResolvedValue({
          conversations: [{ ...summary, channelId }],
          complete: true,
        }),
      },
      () => "id",
    )
    await expect(refused.list(), String(channelId)).rejects.toThrow(/channelId/)
  }
})

const observeCursor = {
  incarnation: "a".repeat(32),
  boundary: "9",
  creation: "10",
  id: conversationId,
}

it("observes one catalogue page and keeps an unfinished page unfinished", async () => {
  const request = vi
    .fn()
    .mockResolvedValueOnce({
      conversations: [summary],
      complete: false,
      cursor: observeCursor,
    })
    .mockResolvedValueOnce({ conversations: [summary], complete: false })
    .mockResolvedValue({ conversations: [summary], complete: true })
  const api = createConversationApi({ request }, () => "id")
  const page = await api.observe({ cursor: observeCursor })
  expect(page.complete).toBe(false)
  expect(page.cursor).toEqual(observeCursor)
  expect(request).toHaveBeenCalledWith("conversation.observe", { cursor: observeCursor })
  const unfinished = await api.observe()
  expect(unfinished.complete).toBe(false)
  expect(unfinished.cursor).toBeUndefined()
  expect(request).toHaveBeenLastCalledWith("conversation.observe", {})
  await expect(api.observe({ archived: true })).rejects.toThrow(/archived filter/)
})

it("refuses an observation page past the catalogue bound, or a cursor that does not advance as a decimal", async () => {
  const over = createConversationApi(
    {
      request: vi.fn().mockResolvedValue({
        conversations: Array.from(
          { length: bounds.maxCataloguePageEntries + 1 },
          (_, index) => ({
            ...summary,
            conversationId: `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
          }),
        ),
        complete: false,
        cursor: observeCursor,
      }),
    },
    () => "id",
  )
  await expect(over.observe()).rejects.toThrow(/conversations/)
  const request = vi.fn().mockResolvedValue({ conversations: [], complete: true })
  const api = createConversationApi({ request }, () => "id")
  await expect(
    api.observe({ cursor: { ...observeCursor, creation: "01" } }),
  ).rejects.toThrow(/cursor/i)
  expect(request).not.toHaveBeenCalled()
  await expect(api.observe({ cursor: observeCursor })).resolves.toMatchObject({
    complete: true,
  })
})

it("reads a deleted conversation as refused and an unfinished erasure as a delete that happened", async () => {
  const rejecting = (code: string) =>
    createConversationApi(
      { request: vi.fn().mockRejectedValue(new NessaRpcError(code, code)) },
      () => "action",
    )
  const refused = await rejecting(ConversationErrorCode.ConversationDeleted)
    .close(conversationId)
    .catch((error: unknown) => error)
  expect(refused).toBeInstanceOf(NessaConversationControlError)
  expect((refused as NessaConversationControlError).uncertain).toBe(false)
  const unfinished = await rejecting(ConversationErrorCode.ConversationErasureIncomplete)
    .delete(conversationId)
    .catch((error: unknown) => error)
  expect((unfinished as NessaConversationControlError).code).toBe(
    ConversationErrorCode.ConversationErasureIncomplete,
  )
  expect((unfinished as NessaConversationControlError).uncertain).toBe(true)
})

it("reads a view's title as the gateway's, null before anything was said, and refuses an empty one", async () => {
  const reading = (title: unknown) =>
    createConversationApi(
      { request: vi.fn().mockResolvedValue({ ...view, title }) },
      () => "id",
    ).read(conversationId)
  await expect(reading(null)).resolves.toMatchObject({ title: null })
  await expect(reading("")).rejects.toThrow(/title/)
  await expect(reading("😀".repeat(65))).rejects.toThrow(/title/)
  await expect(reading(undefined)).rejects.toThrow(/title/)
})

it("waits out one delete's worst case, and only for delete", async () => {
  const request = vi.fn(
    async (_method: string, params: unknown, _deadline?: unknown) => ({
      requestId: (params as { requestId: string }).requestId,
      applied: true,
    }),
  )
  const api = createConversationApi({ request }, () => "action")
  await api.delete(conversationId)
  await api.close(conversationId)
  expect(request.mock.calls[0]![2]).toEqual({ atLeastMs: conversationDeleteTimeoutMs })
  expect(request.mock.calls[1]![2]).toBeUndefined()
})

const note = {
  messageId: "2f1d1c1e-3b7a-4c7e-9a51-0c1e2d3f4a5b",
  text: "Use the May fares",
  acceptedAtMs: 1_700_000_000_000,
  delivery: "next_turn",
  deliveryState: "assigned",
  executionId: "run-2",
  afterExecutionId: "run-1",
  author: { kind: "grant", surface: "phone" },
} as const

it("leaves a note under a stable request and retries it as the same note", async () => {
  const receipt = {
    requestId: "note-1",
    messageId: note.messageId,
    acceptedAtMs: note.acceptedAtMs,
    deliveryState: "pending",
  }
  const request = vi
    .fn()
    .mockRejectedValueOnce(new Error("connection lost"))
    .mockResolvedValue(receipt)
  const api = createConversationApi({ request }, () => "generated")
  const failure = await api
    .message(conversationId, "Use the May fares", "next_turn", { requestId: "note-1" })
    .catch((error: unknown) => error)
  expect(failure).toBeInstanceOf(NessaConversationMutationError)
  await expect(
    (failure as NessaConversationMutationError<unknown>).retry(),
  ).resolves.toEqual(receipt)
  expect(request).toHaveBeenCalledTimes(2)
  for (const call of request.mock.calls)
    expect(call).toEqual([
      "conversation.message",
      {
        conversationId,
        requestId: "note-1",
        text: "Use the May fares",
        delivery: "next_turn",
      },
    ])
  // A generated request identity when none is given.
  await api.message(conversationId, "fyi", "record_only").catch(() => undefined)
  expect(request.mock.calls[2][1]).toMatchObject({
    requestId: "generated",
    delivery: "record_only",
  })
})

it("refuses a note outside its bounds before anything is sent", () => {
  const request = vi.fn()
  const api = createConversationApi({ request }, () => "generated")
  for (const text of ["", "  \n", "x".repeat(8193)])
    expect(() => api.message(conversationId, text, "next_turn")).toThrow(TypeError)
  expect(() =>
    api.message(conversationId, "x", "later" as unknown as "next_turn"),
  ).toThrow(TypeError)
  expect(() => api.message("not-a-uuid", "x", "record_only")).toThrow(TypeError)
  expect(request).not.toHaveBeenCalled()
})

it("accepts a note receipt only for its request and with a coherent state", async () => {
  const base = {
    requestId: "note-1",
    messageId: note.messageId,
    acceptedAtMs: note.acceptedAtMs,
  }
  for (const valid of [
    { ...base, deliveryState: "recorded" },
    { ...base, deliveryState: "pending" },
    { ...base, deliveryState: "assigned", executionId: "run-2" },
    { ...base, deliveryState: "retired", retiredReason: "permission_lost" },
  ]) {
    const api = createConversationApi(
      { request: vi.fn().mockResolvedValue(valid) },
      () => "generated",
    )
    await expect(
      api.message(conversationId, "x", "next_turn", { requestId: "note-1" }),
    ).resolves.toEqual(valid)
  }
  for (const invalid of [
    { ...base, requestId: "another", deliveryState: "pending" },
    { ...base, deliveryState: "delivered" },
    { ...base, deliveryState: "assigned" },
    { ...base, deliveryState: "pending", executionId: "run-2" },
    { ...base, deliveryState: "retired" },
    { ...base, deliveryState: "retired", retiredReason: "stale" },
    { ...base, deliveryState: "pending", acceptedAtMs: -1 },
    { ...base, deliveryState: "pending", extra: true },
  ]) {
    const api = createConversationApi(
      { request: vi.fn().mockResolvedValue(invalid) },
      () => "generated",
    )
    await expect(
      api.message(conversationId, "x", "next_turn", { requestId: "note-1" }),
    ).rejects.toBeInstanceOf(NessaConversationMutationError)
  }
})

it("reads where the caller's note stands and refuses a status that contradicts itself", async () => {
  const found = {
    found: true,
    requestId: "note-1",
    messageId: note.messageId,
    acceptedAtMs: note.acceptedAtMs,
    deliveryState: "assigned",
    executionId: "run-2",
  }
  const request = vi.fn().mockResolvedValue(found)
  const api = createConversationApi({ request }, () => "generated")
  await expect(api.messageStatus(conversationId, "note-1")).resolves.toEqual(found)
  expect(request).toHaveBeenCalledWith("conversation.messageStatus", {
    conversationId,
    requestId: "note-1",
  })
  const missing = { found: false, requestId: "note-1" }
  await expect(
    createConversationApi(
      { request: vi.fn().mockResolvedValue(missing) },
      () => "generated",
    ).messageStatus(conversationId, "note-1"),
  ).resolves.toEqual(missing)
  for (const invalid of [
    { ...missing, deliveryState: "pending" },
    { ...found, requestId: "another" },
    { ...found, executionId: undefined },
  ]) {
    await expect(
      createConversationApi(
        { request: vi.fn().mockResolvedValue(invalid) },
        () => "generated",
      ).messageStatus(conversationId, "note-1"),
    ).rejects.toThrow()
  }
  await expect(api.messageStatus(conversationId, "")).rejects.toThrow(TypeError)
})

it("accepts a view's bounded notes and refuses an incoherent one", async () => {
  const read = (notes: unknown[]) =>
    createConversationApi(
      { request: vi.fn().mockResolvedValue({ ...view, notes }) },
      () => "generated",
    ).read(conversationId)
  await expect(read([note])).resolves.toMatchObject({ notes: [note] })
  await expect(read(Array(64).fill(note))).resolves.toBeDefined()
  for (const invalid of [
    Array(65).fill(note),
    [{ ...note, text: "" }],
    [{ ...note, text: "x".repeat(8193) }],
    [{ ...note, deliveryState: "pending" }],
    [{ ...note, author: { kind: "app", surface: "phone" } }],
    [{ ...note, author: { kind: "owner", surface: "" } }],
    [{ ...note, delivery: "later" }],
    [{ ...note, policyRevision: 3 }],
  ])
    await expect(read(invalid)).rejects.toThrow()
  await expect(
    createConversationApi(
      { request: vi.fn().mockResolvedValue({ ...view, notes: undefined }) },
      () => "generated",
    ).read(conversationId),
  ).rejects.toThrow()
})

const credentialId = "device-credential"
const drivePolicy: ConversationToolPolicy = {
  preset: "read_only_tools",
  commandHosts: ["devbox"],
}

it("grants, changes and revokes a share under a stable request id", async () => {
  const request = vi.fn(async (method: string, params: unknown) => ({
    requestId: (params as { requestId: string }).requestId,
    applied: true,
    ...(method === "conversation.share" ? { policyRevision: 7 } : {}),
  }))
  const api = createConversationApi({ request }, () => "action")
  const policy = { preset: "read_only_tools" as const, commandHosts: ["devbox"] }
  expect(
    await api.share(conversationId, credentialId, ["read", "comment", "drive"], policy),
  ).toEqual({
    requestId: "action",
    applied: true,
    policyRevision: 7,
  })
  // Changing the caller's own object after the call changes nothing sent.
  policy.commandHosts.push("other")
  await api.share(conversationId, credentialId, ["read"], undefined, {
    expectedPolicyRevision: 7,
  })
  await api.unshare(conversationId, credentialId, { requestId: "revoke" })
  await api.unshare(conversationId, credentialId, {
    requestId: "revoke-built",
    expectedPolicyRevision: 0,
  })
  expect(request.mock.calls).toEqual([
    [
      "conversation.share",
      {
        conversationId,
        requestId: "action",
        credentialId,
        capabilities: ["read", "comment", "drive"],
        policy: drivePolicy,
      },
    ],
    [
      "conversation.share",
      {
        conversationId,
        requestId: "action",
        credentialId,
        capabilities: ["read"],
        expectedPolicyRevision: 7,
      },
    ],
    ["conversation.unshare", { conversationId, requestId: "revoke", credentialId }],
    [
      "conversation.unshare",
      {
        conversationId,
        requestId: "revoke-built",
        credentialId,
        expectedPolicyRevision: 0,
      },
    ],
  ])
})

it("refuses a malformed share before anything is sent", async () => {
  const request = vi.fn()
  const api = createConversationApi({ request }, () => "action")
  expect(() => api.share("not-a-uuid", credentialId, ["read"])).toThrow(TypeError)
  expect(() => api.share(conversationId, " ", ["read"])).toThrow(TypeError)
  expect(() => api.share(conversationId, "c".repeat(257), ["read"])).toThrow(TypeError)
  expect(() => api.unshare(conversationId, "")).toThrow(TypeError)
  expect(() =>
    api.share(conversationId, credentialId, ["read"], undefined, { requestId: "" }),
  ).toThrow(TypeError)
  for (const expectedPolicyRevision of [-1, 1.5, Number.NaN, 2 ** 53]) {
    expect(() =>
      api.share(conversationId, credentialId, ["read"], undefined, {
        expectedPolicyRevision,
      }),
    ).toThrow(TypeError)
    expect(() =>
      api.unshare(conversationId, credentialId, { expectedPolicyRevision }),
    ).toThrow(TypeError)
  }
  expect(request).not.toHaveBeenCalled()
})

it("keeps a share's typed refusal and retries it as the same grant", async () => {
  const request = vi
    .fn()
    .mockRejectedValueOnce(
      new NessaRpcError("share_target_not_paired", "share_target_not_paired"),
    )
    .mockResolvedValueOnce({ requestId: "action", applied: false, policyRevision: 3 })
  const api = createConversationApi({ request }, () => "action")
  const refused = await api
    .share(conversationId, credentialId, ["read", "comment", "drive"], drivePolicy)
    .catch((error: unknown) => error)
  expect(refused).toBeInstanceOf(NessaConversationMutationError)
  const error = refused as NessaConversationMutationError<unknown>
  expect(error.code).toBe(ConversationErrorCode.ShareTargetNotPaired)
  expect(error.uncertain).toBe(false)
  expect(await error.retry()).toEqual({
    requestId: "action",
    applied: false,
    policyRevision: 3,
  })
  expect(request.mock.calls[1]).toEqual(request.mock.calls[0])
})

it("keeps a refusal for a grant changed since its revision as its own code", async () => {
  const request = vi
    .fn()
    .mockRejectedValue(new NessaRpcError("share_grant_changed", "share_grant_changed"))
  const api = createConversationApi({ request }, () => "action")
  for (const call of [
    () =>
      api.share(conversationId, credentialId, ["read"], undefined, {
        expectedPolicyRevision: 2,
      }),
    () => api.unshare(conversationId, credentialId, { expectedPolicyRevision: 2 }),
  ]) {
    const error = (await call().catch(
      (caught: unknown) => caught,
    )) as NessaConversationMutationError<unknown>
    expect(error).toBeInstanceOf(NessaConversationMutationError)
    expect(error.code).toBe(ConversationErrorCode.ShareGrantChanged)
    expect(error.uncertain).toBe(false)
  }
})

it.each([
  ["another request", { requestId: "other", applied: true, policyRevision: 1 }],
  ["no revision", { requestId: "action", applied: true }],
  ["a negative revision", { requestId: "action", applied: true, policyRevision: -1 }],
  [
    "revision 0, which no grant holds",
    { requestId: "action", applied: true, policyRevision: 0 },
  ],
  ["a fractional revision", { requestId: "action", applied: true, policyRevision: 1.5 }],
  [
    "an unknown field",
    { requestId: "action", applied: true, policyRevision: 1, extra: true },
  ],
])("rejects a share answer with %s", async (_name, answer) => {
  const api = createConversationApi(
    { request: vi.fn().mockResolvedValue(answer) },
    () => "action",
  )
  await expect(api.share(conversationId, credentialId, ["read"])).rejects.toThrow(
    NessaConversationMutationError,
  )
})

it("rejects an unshare answer for another request", async () => {
  const api = createConversationApi(
    { request: vi.fn().mockResolvedValue({ requestId: "other", applied: true }) },
    () => "action",
  )
  await expect(api.unshare(conversationId, credentialId)).rejects.toThrow(
    NessaConversationMutationError,
  )
})

const grant = {
  credentialId,
  capabilities: ["read", "comment", "drive"] as const,
  policy: { allow: ["Read"], deny: [], commandHosts: ["devbox"] },
  policyRevision: 2,
  grantedAtMs: 1_700_000_000_000,
}

const page = { pageSize: bounds.maxSharePage }

it("lists one page of a conversation's grants within the published bounds", async () => {
  const request = vi.fn().mockResolvedValue({
    changeRevision: 1,
    items: [
      grant,
      {
        credentialId: "phone",
        capabilities: ["read"],
        policyRevision: 1,
        grantedAtMs: 1,
      },
    ],
  })
  const api = createConversationApi({ request }, () => "action")
  expect((await api.shares(conversationId, page)).items).toHaveLength(2)
  expect(request).toHaveBeenCalledWith("conversation.shares", {
    conversationId,
    pageSize: bounds.maxSharePage,
  })
  // A full allow list, a full deny list, and every host, including one at the
  // longest destination config.json accepts.
  request.mockResolvedValue({
    changeRevision: 1,
    items: [
      {
        ...grant,
        policy: {
          allow: Array.from({ length: bounds.maxToolPolicyPatterns }, (_, n) => `a${n}`),
          deny: Array.from({ length: bounds.maxToolPolicyPatterns }, (_, n) => `d${n}`),
          commandHosts: [
            ...Array.from({ length: bounds.maxCommandHosts - 1 }, (_, n) => `h${n}`),
            "h".repeat(bounds.maxCommandHostBytes),
          ],
        },
      },
    ],
  })
  await expect(api.shares(conversationId, page)).resolves.toBeDefined()
  const cursor = { grantedAtMs: grant.grantedAtMs, credentialId: grant.credentialId }
  request.mockResolvedValue({ changeRevision: 1, items: [grant], cursor })
  await api.shares(conversationId, { pageSize: 1, cursor })
  expect(request).toHaveBeenLastCalledWith("conversation.shares", {
    conversationId,
    pageSize: 1,
    cursor,
  })
})

it("refuses a shares answer larger than the page size it asked for", async () => {
  const request = vi.fn().mockResolvedValue({
    changeRevision: 1,
    items: [
      grant,
      {
        credentialId: "phone",
        capabilities: ["read"],
        policyRevision: 1,
        grantedAtMs: 1,
      },
    ],
  })
  const api = createConversationApi({ request }, () => "action")
  await expect(api.shares(conversationId, { pageSize: 1 })).rejects.toThrow(/page/i)
  expect(request).toHaveBeenCalledOnce()
})

it("refuses a page size outside the published bound before sending", async () => {
  const request = vi.fn()
  const api = createConversationApi({ request }, () => "action")
  await expect(api.shares(conversationId, { pageSize: 0 })).rejects.toThrow(TypeError)
  await expect(
    api.shares(conversationId, { pageSize: bounds.maxSharePage + 1 }),
  ).rejects.toThrow(TypeError)
  expect(request).not.toHaveBeenCalled()
})

it.each([
  ["a repeated grant", { items: [grant, grant] }],
  [
    "a grant at revision 0, which names no grant held",
    { items: [{ ...grant, policyRevision: 0 }] },
  ],
  ["a drive grant without a policy", { items: [{ ...grant, policy: undefined }] }],
  ["a read grant with a policy", { items: [{ ...grant, capabilities: ["read"] }] }],
  [
    "a set that is not one of the three",
    { items: [{ ...grant, capabilities: ["read", "drive"] }] },
  ],
  ["an unknown capability", { items: [{ ...grant, capabilities: ["owner"] }] }],
  [
    "a preset beside patterns",
    { items: [{ ...grant, policy: { preset: "all", allow: ["Read"] } }] },
  ],
  ["an unknown preset", { items: [{ ...grant, policy: { preset: "anything" } }] }],
  ["an empty policy", { items: [{ ...grant, policy: { allow: [], deny: [] } }] }],
  [
    "an empty host list",
    { items: [{ ...grant, policy: { preset: "all", commandHosts: [] } }] },
  ],
  [
    "a host named twice",
    { items: [{ ...grant, policy: { preset: "all", commandHosts: ["a", "a"] } }] },
  ],
  [
    "a host past its bytes",
    {
      items: [
        {
          ...grant,
          policy: {
            preset: "all",
            commandHosts: ["h".repeat(bounds.maxCommandHostBytes + 1)],
          },
        },
      ],
    },
  ],
  ["an unknown field", { items: [{ ...grant, extra: true }] }],
  ["a negative revision", { items: [{ ...grant, policyRevision: -1 }] }],
  [
    "more grants than a page holds",
    {
      items: Array.from({ length: bounds.maxSharePage + 1 }, (_, n) => ({
        credentialId: `c${n}`,
        capabilities: ["read"],
        policyRevision: 1,
        grantedAtMs: n,
      })),
    },
  ],
  [
    "a cursor when the page is empty",
    { items: [], cursor: { grantedAtMs: 1, credentialId } },
  ],
  [
    "a cursor that is not the page's last grant",
    {
      items: [grant],
      cursor: { grantedAtMs: grant.grantedAtMs, credentialId: "other" },
    },
  ],
  ["no catalogue revision", { items: [grant] }],
  [
    "a catalogue revision that is not a safe integer",
    { items: [grant], changeRevision: 1.5 },
  ],
])("rejects %s in a shares answer", async (name, answer) => {
  const payload =
    name === "no catalogue revision" ? answer : { changeRevision: 1, ...answer }
  const api = createConversationApi(
    { request: vi.fn().mockResolvedValue(payload) },
    () => "action",
  )
  await expect(api.shares(conversationId, page)).rejects.toThrow()
})

it("correlates a model switch and validates its whole acknowledged selection", async () => {
  const selected = {
    requestId: "switch-1",
    model: "claude-haiku-4-5-20251001",
    approvalMode: "ask",
    effect: "subsequentResponses",
  }
  const request = vi.fn().mockResolvedValue(selected)
  const api = createConversationApi({ request }, () => "generated")
  await expect(
    api.setModel(conversationId, selected.model, { requestId: selected.requestId }),
  ).resolves.toEqual(selected)
  expect(request).toHaveBeenCalledExactlyOnceWith("conversation.setModel", {
    conversationId,
    requestId: selected.requestId,
    model: selected.model,
  })
  for (const bad of [
    { ...selected, requestId: "another-action" },
    { ...selected, model: "another-model" },
    { ...selected, approvalMode: "invented" },
    { ...selected, effect: "immediate" },
  ]) {
    request.mockResolvedValue(bad)
    await expect(
      api.setModel(conversationId, selected.model, { requestId: selected.requestId }),
    ).rejects.toBeInstanceOf(NessaConversationControlError)
  }
})

it("reads all a waiting question asks range by range, and refuses another question's", async () => {
  const key = "0123456789abcdef0123456789abcdef"
  const asked = {
    question: key,
    executionId: "execution",
    questionId: "ask",
    message: '"'.repeat(1024),
    questions: [
      {
        key: "environment",
        prompt: '"'.repeat(1024),
        header: null,
        multiSelect: false,
        freeText: false,
        required: true,
        options: [{ value: "staging", label: '"'.repeat(1024), description: null }],
      },
    ],
  }
  const serve = (value: unknown) => {
    const encoded = new TextEncoder().encode(JSON.stringify(value))
    return vi.fn(async (_method: string, params: unknown) => {
      const { offset, maxBytes } = params as { offset: number; maxBytes: number }
      let binary = ""
      for (const byte of encoded.subarray(offset, offset + maxBytes))
        binary += String.fromCharCode(byte)
      return { offset, totalBytes: encoded.byteLength, data: btoa(binary) }
    })
  }
  const request = serve(asked)
  const api = createConversationApi({ request }, () => "unused")
  await expect(api.questionReview(conversationId, key)).resolves.toEqual(asked)
  expect(request.mock.calls[0]).toEqual([
    "conversation.questionReview",
    { conversationId, question: key, offset: 0, maxBytes: 32_768 },
  ])
  await expect(
    createConversationApi(
      { request: serve({ ...asked, question: "f".repeat(32) }) },
      () => "unused",
    ).questionReview(conversationId, key),
  ).rejects.toThrow("another question")
  const unasked = serve(asked)
  await expect(
    createConversationApi({ request: unasked }, () => "unused").questionReview(
      conversationId,
      "ask",
    ),
  ).rejects.toThrow()
  expect(unasked).not.toHaveBeenCalled()
})

it("reads what a waiting review shows range by range, and refuses one that does not add up", async () => {
  // A review of a 100 KB Write input, of two-byte characters split across ranges.
  const key = "0123456789abcdef0123456789abcdef"
  const review: ConversationPermission = {
    review: key,
    executionId: "execution",
    permissionId: "review",
    toolId: "tool",
    title: "Write",
    toolName: "Write",
    argumentsJson: JSON.stringify({
      file_path: "/tmp/big.txt",
      content: "é".repeat(50_000),
    }),
    origin: { kind: "harness" },
    ask: "tool",
    options: [{ id: "allow", label: "Allow once", effect: "allow" }],
  }
  const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value))
  const bytes = encode(review)
  const serve = (encoded: Uint8Array) =>
    vi.fn(async (_method: string, params: unknown) => {
      const { offset, maxBytes } = params as { offset: number; maxBytes: number }
      const part = encoded.subarray(offset, offset + maxBytes)
      let binary = ""
      for (const byte of part) binary += String.fromCharCode(byte)
      return { offset, totalBytes: encoded.byteLength, data: btoa(binary) }
    })
  const request = serve(bytes)
  const api = createConversationApi({ request }, () => "unused")
  await expect(api.permissionReview(conversationId, key)).resolves.toEqual(review)
  const asked = request.mock.calls.map((call) => call[1] as Record<string, unknown>)
  expect(asked.length).toBe(Math.ceil(bytes.byteLength / 32_768))
  expect(asked[0]).toEqual({ conversationId, review: key, offset: 0, maxBytes: 32_768 })
  expect(asked.map((each) => each.offset)).toEqual(
    asked.map((_, index) => index * 32_768),
  )
  expect(
    request.mock.calls.every((call) => call[0] === "conversation.permissionReview"),
  ).toBe(true)
  const refused = (encoded: Uint8Array) =>
    createConversationApi({ request: serve(encoded) }, () => "unused").permissionReview(
      conversationId,
      key,
    )
  // Bytes that are not UTF-8, or not JSON, or not a review, are not a review.
  await expect(refused(new Uint8Array([0xff]))).rejects.toThrow()
  await expect(refused(new TextEncoder().encode("{"))).rejects.toThrow()
  await expect(refused(encode({ ...review, options: [] }))).rejects.toThrow()
  await expect(
    refused(encode({ ...review, options: [review.options[0], review.options[0]] })),
  ).rejects.toThrow("repeats an option")
  await expect(refused(encode({ ...review, argumentsComplete: true }))).rejects.toThrow()
  // Another review's whole, under another key, is not the review asked for.
  await expect(refused(encode({ ...review, review: "f".repeat(32) }))).rejects.toThrow(
    "another review",
  )
  // A key that is not one a view lists is not asked for.
  const unasked = serve(bytes)
  await expect(
    createConversationApi({ request: unasked }, () => "unused").permissionReview(
      conversationId,
      "review",
    ),
  ).rejects.toThrow()
  // A read let go is not asked again.
  const stopped = new AbortController()
  stopped.abort()
  await expect(
    createConversationApi({ request: unasked }, () => "unused").permissionReview(
      conversationId,
      key,
      { signal: stopped.signal },
    ),
  ).rejects.toThrow()
  expect(unasked).not.toHaveBeenCalled()
})

it.each([
  [ConversationErrorCode.ModelChangeNotApplied, false],
  [ConversationErrorCode.ModelChangeUncertain, true],
])("retains model refusal certainty for %s", async (code, uncertain) => {
  const cause = new NessaRpcError(code, code)
  const request = vi.fn().mockRejectedValue(cause)
  const api = createConversationApi({ request }, () => "model-control")
  const error = await api
    .setModel(conversationId, "selected-model", { requestId: "original-model-control" })
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(NessaConversationControlError)
  expect(error).toMatchObject({
    code,
    uncertain,
    requestId: "original-model-control",
    cause,
  })
  expect(request).toHaveBeenCalledExactlyOnceWith("conversation.setModel", {
    conversationId,
    requestId: "original-model-control",
    model: "selected-model",
  })
})

it("captures file-only uploads immutably and includes them in the admitted request", async () => {
  const request = vi
    .fn()
    .mockResolvedValue({ executionId: "execution", disposition: "queued" })
  const api = createConversationApi({ request }, () => "request")
  const upload = {
    digest: `sha256:${"a".repeat(64)}`,
    mimeType: "application/pdf",
    size: 12,
    name: "report.pdf",
  }
  const prepared = api.prepare(conversationId, "", [], [], {
    executionId: "execution",
    uploads: [upload],
  })
  upload.name = "changed.pdf"
  expect(prepared.uploads[0]?.name).toBe("report.pdf")
  expect(Object.isFrozen(prepared.uploads[0])).toBe(true)
  await api.submit(prepared)
  expect(request.mock.calls[0]?.[1]).toMatchObject({
    uploads: [{ name: "report.pdf" }],
    attachments: [],
    files: [],
  })
  expect(() =>
    api.prepare(conversationId, "", [], [], {
      uploads: [{ ...upload, name: "../escape" }],
    }),
  ).toThrow("filename")
})
