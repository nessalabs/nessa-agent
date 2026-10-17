import {
  prepareSubmission,
  permissionTargets,
  questionTargets,
  executionTargets,
  type PreparedSubmission,
} from "@nessa/client"
import { expect, it, vi } from "vitest"
import { createDependencies } from "../../../composition/dependencies"
import { makeStore } from "../../../store"
import { scenarioEffects } from "../scenario/effects"
import { textContent } from "../../model"
import {
  ControlFailedError,
  SubmissionRefusedError,
  type ConversationEffects,
} from "../../application/ports"
import type { ConversationView } from "../../application/view"
import {
  bindConversation,
  closeTab,
  closeConversation,
  controlConversation,
  followConversation,
  openConversation,
  sendDraft,
  setActive,
  setDraft,
  submissionStarted,
  submissionAccepted,
  stopGenerating,
  unfollowConversation,
  type Control,
} from "./slice"
import { PreparedOperationTable } from "./prepared-operation-table"

const first = "00000000-0000-4000-8000-000000000001"
const second = "00000000-0000-4000-8000-000000000002"
const sameExecution = "00000000-0000-4000-8000-000000000003"
function questionTarget() {
  return questionTargets({
    conversationId: first,
    questions: [
      {
        executionId: "original",
        questionId: "question",
        message: "Choose",
        questions: [
          {
            key: "key",
            prompt: "Choose",
            required: false,
            multiSelect: false,
            freeText: true,
            options: [{ value: "original", label: "Original" }],
          },
        ],
      },
    ],
  })[0]!
}
function gate() {
  let resolve!: () => void
  const promise = new Promise<void>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
function acceptedView(conversationId: string, executionId: string): ConversationView {
  return {
    conversationId,
    artifacts: [],
    notes: [],
    unreadable: [],
    unreadableTruncated: false,
    approvalMode: "ask",
    approvalModes: [
      { id: "ask", name: "Provider asks", description: "Provider asks where required." },
    ],
    title: null,
    revision: "accepted",
    truncated: false,
    queueComplete: true,
    transcriptState: "complete",
    messages: [
      {
        executionId,
        parts: [],
        userText: "original",
        attachments: [],
        files: [],
        status: "queued",
      },
    ],
    reviewsWaiting: [],
    permissions: [],
    questions: [],
    tools: [],
    pending: [],
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
  }
}
function fixture(
  overrides: Partial<ConversationEffects> = {},
  echo = scenarioEffects("echo"),
) {
  const stop = vi.fn<ConversationEffects["stop"]>(async () => {})
  const remove = vi.fn<ConversationEffects["remove"]>(async () => {})
  const submit = vi.fn(echo.submit)
  const effects = { ...echo, stop, remove, submit, ...overrides }
  const store = makeStore(createDependencies({ conversation: effects }))
  store.dispatch(bindConversation({ id: "c0", serverId: first }))
  return { store, echo, effects, stop, remove, submit }
}
function targetOf(store: ReturnType<typeof makeStore>, id = "c0") {
  const conversation = store
    .getState()
    .conversation.conversations.find((conversation) => conversation.id === id)!
  return executionTargets({
    conversationId: conversation.serverConversationId!,
    messages: conversation.turns.flatMap((turn) =>
      turn.executionId ? [{ executionId: turn.executionId }] : [],
    ),
    pending: [],
  })[0]!
}

function watchPreparedOperations() {
  const prune = vi.spyOn(PreparedOperationTable.prototype, "prune")
  return {
    table: () => prune.mock.contexts[0] as PreparedOperationTable,
    restore: () => prune.mockRestore(),
  }
}

it("prunes a settled unknown retry operation when its tab closes", async () => {
  const watched = watchPreparedOperations()
  try {
    const echo = scenarioEffects("echo")
    const submit = vi
      .fn(echo.submit)
      .mockRejectedValueOnce(new Error("lost acknowledgement"))
    const { store } = fixture({ submit, create: echo.create }, echo)
    await store.dispatch(sendDraft({ content: textContent("original") }))
    const prepared = submit.mock.calls[0]![0]
    expect(watched.table().get("c0", prepared.executionId)).toBeDefined()

    await store.dispatch(closeTab("c0"))
    expect(watched.table().get("c0", prepared.executionId)).toBeUndefined()
  } finally {
    watched.restore()
  }
})

it("prunes a closed tab's retry operation when an uncertain send settles", async () => {
  const watched = watchPreparedOperations()
  try {
    const waiting = gate()
    const echo = scenarioEffects("echo")
    const submit = vi.fn(async (_prepared: PreparedSubmission) => {
      await waiting.promise
      throw new Error("lost acknowledgement")
    })
    const { store } = fixture({ submit, create: echo.create }, echo)
    const sending = store.dispatch(sendDraft({ content: textContent("original") }))
    await vi.waitFor(() => expect(submit).toHaveBeenCalledOnce())
    const prepared = submit.mock.calls[0]![0]
    await store.dispatch(closeTab("c0"))
    expect(watched.table().get("c0", prepared.executionId)).toBeUndefined()

    waiting.resolve()
    await sending
    expect(watched.table().get("c0", prepared.executionId)).toBeUndefined()
  } finally {
    watched.restore()
  }
})

it("prunes retry intent as soon as a passive authoritative view accepts the turn", async () => {
  const watched = watchPreparedOperations()
  try {
    const echo = scenarioEffects("echo")
    let publishView: ((view: ConversationView) => void) | undefined
    const stopFollowing = vi.fn()
    const follow: ConversationEffects["follow"] = (_id, listener) => {
      publishView = listener.view
      return stopFollowing
    }
    const submit = vi.fn<ConversationEffects["submit"]>(async () => {
      throw new Error("lost acknowledgement")
    })
    const { store } = fixture({ follow, submit }, echo)
    const following = store.dispatch(followConversation("c0"))
    await vi.waitFor(() => expect(publishView).toBeDefined())
    const sent = await store.dispatch(sendDraft({ content: textContent("original") }))
    expect(sent.meta.requestStatus).toBe("rejected")
    const prepared = submit.mock.calls[0]![0]
    expect(watched.table().get("c0", prepared.executionId)).toBeDefined()

    publishView!(acceptedView(first, prepared.executionId))
    await vi.waitFor(() =>
      expect(store.getState().conversation.conversations[0]!.turns[0]).toMatchObject({
        receipt: "accepted",
      }),
    )
    expect(watched.table().get("c0", prepared.executionId)).toBeUndefined()
    await store.dispatch(unfollowConversation("c0"))
    await following
    expect(stopFollowing).toHaveBeenCalledOnce()
  } finally {
    watched.restore()
  }
})

it.each([false, true])(
  "keeps a retry stoppable after a passive view accepts it during creation (PS28 refused=%s)",
  async (stopRefused) => {
    const waiting = gate()
    const echo = scenarioEffects("echo")
    let creates = 0
    let follows = 0
    const retry = { executionId: undefined as string | undefined }
    let publishView: ((view: ConversationView) => void) | undefined
    const stopFollowing = vi.fn()
    const follow = vi.fn<ConversationEffects["follow"]>((_id, listener) => {
      follows++
      publishView = listener.view
      if (follows > 2 && retry.executionId)
        listener.view(acceptedView(first, retry.executionId))
      return stopFollowing
    })
    const create = vi.fn(async (id: string) => {
      if (creates++ === 1) await waiting.promise
      return echo.create(id)
    })
    const submit = vi
      .fn(echo.submit)
      .mockRejectedValueOnce(new Error("lost acknowledgement"))
    const stop = vi.fn<ConversationEffects["stop"]>(async () => {})
    if (stopRefused)
      stop.mockRejectedValueOnce(
        new ControlFailedError("conversation-not-found", "refused"),
      )
    const { store } = fixture({ create, follow, submit, stop }, echo)
    const following = store.dispatch(followConversation("c0"))
    await vi.waitFor(() => expect(publishView).toBeDefined())
    await store.dispatch(sendDraft({ content: textContent("original") }))
    const prepared = submit.mock.calls[0]![0]
    retry.executionId = prepared.executionId

    const retrying = store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "retry", target: prepared.target },
      }),
    )
    await vi.waitFor(() => expect(create).toHaveBeenCalledTimes(2))
    const refreshing = store.dispatch(followConversation("c0"))
    await vi.waitFor(() => expect(follow).toHaveBeenCalledTimes(2))
    publishView!(acceptedView(first, prepared.executionId))
    await vi.waitFor(() =>
      expect(store.getState().conversation.conversations[0]!.turns[0]).toMatchObject({
        receipt: "accepted",
      }),
    )
    const stopping = await store.dispatch(stopGenerating({ conversationId: "c0" }))
    expect(stopping.meta.requestStatus).toBe(stopRefused ? "rejected" : "fulfilled")
    expect(stop).toHaveBeenCalledOnce()

    waiting.resolve()
    const result = await retrying
    expect(result.meta.requestStatus).toBe("fulfilled")
    expect(submit.mock.calls[1]![0]).toBe(prepared)
    expect(stop).toHaveBeenCalledTimes(2)
    expect(stop.mock.calls[0]![0]).toBe(prepared.target)
    expect(stop.mock.calls[1]![0]).toBe(prepared.target)
    await store.dispatch(unfollowConversation("c0"))
    await Promise.all([following, refreshing])
    expect(stopFollowing).toHaveBeenCalled()
  },
)

it("settles retry ownership when creation fails after acceptance and tab closure (PS29)", async () => {
  const watched = watchPreparedOperations()
  try {
    const waiting = gate()
    const echo = scenarioEffects("echo")
    let creates = 0
    let follows = 0
    const retry = { executionId: undefined as string | undefined }
    let publishView: ((view: ConversationView) => void) | undefined
    const stopFollowing = vi.fn()
    const follow = vi.fn<ConversationEffects["follow"]>((_id, listener) => {
      follows++
      publishView = listener.view
      if (follows > 2 && retry.executionId)
        listener.view(acceptedView(first, retry.executionId))
      return stopFollowing
    })
    const create = vi.fn(async (id: string) => {
      if (creates++ === 1) {
        await waiting.promise
        throw new SubmissionRefusedError("conversation-not-found")
      }
      return echo.create(id)
    })
    const submit = vi
      .fn(echo.submit)
      .mockRejectedValueOnce(new Error("lost acknowledgement"))
    const { store, stop } = fixture({ create, follow, submit }, echo)
    const following = store.dispatch(followConversation("c0"))
    await vi.waitFor(() => expect(publishView).toBeDefined())
    await store.dispatch(sendDraft({ content: textContent("original") }))
    const prepared = submit.mock.calls[0]![0]
    retry.executionId = prepared.executionId

    const retrying = store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "retry", target: prepared.target },
      }),
    )
    await vi.waitFor(() => expect(create).toHaveBeenCalledTimes(2))
    const refreshing = store.dispatch(followConversation("c0"))
    await vi.waitFor(() => expect(follow).toHaveBeenCalledTimes(2))
    const operation = watched.table().get("c0", prepared.executionId)
    expect(operation?.inflight).toBe(true)
    publishView!(acceptedView(first, prepared.executionId))
    await vi.waitFor(() =>
      expect(store.getState().conversation.conversations[0]!.turns[0]).toMatchObject({
        receipt: "accepted",
      }),
    )
    await store.dispatch(stopGenerating({ conversationId: "c0" }))
    expect(stop).toHaveBeenCalledOnce()
    store.dispatch(closeConversation("c0"))
    expect(watched.table().get("c0", prepared.executionId)).toBeUndefined()

    waiting.resolve()
    const result = await retrying
    expect(result.meta.requestStatus).toBe("rejected")
    expect(operation?.inflight).toBe(false)
    expect(submit).toHaveBeenCalledOnce()
    await store.dispatch(unfollowConversation("c0"))
    await Promise.all([following, refreshing])
    expect(stopFollowing).toHaveBeenCalled()
  } finally {
    watched.restore()
  }
})

it.each(["create", "admission"] as const)(
  "stops the prepared operation immediately during %s and again after admission (PS6)",
  async (phase) => {
    const waiting = gate()
    const echo = scenarioEffects("echo")
    const create = vi.fn(async (id: string) => {
      if (phase === "create") await waiting.promise
      return echo.create(id)
    })
    const submit = vi.fn(async (prepared: PreparedSubmission) => {
      if (phase === "admission") await waiting.promise
      return echo.submit(prepared)
    })
    const { store, stop } = fixture({ create, submit }, echo)
    const sending = store.dispatch(
      sendDraft({ id: "c0", content: textContent("original") }),
    )
    if (phase === "admission")
      await vi.waitFor(() => expect(submit).toHaveBeenCalledOnce())
    const stopping = store.dispatch(stopGenerating({ conversationId: "c0" }))
    expect(stop).toHaveBeenCalledOnce()
    const captured = stop.mock.calls[0]![0]
    expect(captured).toMatchObject({ conversationId: first })
    // The operation promise retains the stop even after the tab leaves the table.
    store.dispatch(closeConversation("c0"))
    await stopping
    waiting.resolve()
    await sending
    expect(stop).toHaveBeenCalledTimes(2)
    expect(stop.mock.calls[1]![0]).toBe(captured)
  },
)

it("same execution identity in two conversations retains stop only on the captured operation (PS7)", async () => {
  const waiting = gate()
  const echo = scenarioEffects("echo")
  const submit = vi.fn(async (prepared: PreparedSubmission) => {
    await waiting.promise
    return echo.submit(prepared)
  })
  const { store, stop } = fixture({ create: echo.create, submit }, echo)
  store.dispatch(openConversation())
  store.dispatch(bindConversation({ id: "c1", serverId: second }))
  const ids = vi.spyOn(crypto, "randomUUID").mockReturnValue(sameExecution)
  try {
    const a = store.dispatch(sendDraft({ id: "c0", content: textContent("first") }))
    const b = store.dispatch(sendDraft({ id: "c1", content: textContent("second") }))
    await vi.waitFor(() => expect(submit).toHaveBeenCalledTimes(2))
    await store.dispatch(stopGenerating({ conversationId: "c0" }))
    waiting.resolve()
    await Promise.all([a, b])
    expect(stop).toHaveBeenCalledTimes(2)
    expect(stop.mock.calls.map((call) => call[0])).toEqual([
      submit.mock.calls[0]![0].target,
      submit.mock.calls[0]![0].target,
    ])
    expect(stop.mock.calls.every((call) => call[0].conversationId === first)).toBe(true)
  } finally {
    ids.mockRestore()
  }
})

// cb9121b06b (#537) kept Stop intent in a process-global execution-ID set.
it("two stores sharing dependencies and execution names cannot consume each other's pending stop (PS8)", async () => {
  const one = gate(),
    two = gate()
  const echo = scenarioEffects("echo")
  const stop = vi.fn<ConversationEffects["stop"]>(async () => {})
  let next = 0
  const submit = vi.fn(async (prepared: PreparedSubmission) => {
    await (next++ === 0 ? one.promise : two.promise)
    return echo.submit(prepared)
  })
  const dependencies = createDependencies({ conversation: { ...echo, stop, submit } })
  const a = makeStore(dependencies),
    b = makeStore(dependencies)
  a.dispatch(bindConversation({ id: "c0", serverId: first }))
  b.dispatch(bindConversation({ id: "c0", serverId: first }))
  const ids = vi.spyOn(crypto, "randomUUID").mockReturnValue(sameExecution)
  try {
    const sendingA = a.dispatch(sendDraft({ content: textContent("first store") }))
    const sendingB = b.dispatch(sendDraft({ content: textContent("second store") }))
    await vi.waitFor(() => expect(submit).toHaveBeenCalledTimes(2))
    await a.dispatch(stopGenerating())
    const targetA = stop.mock.calls[0]![0]
    two.resolve()
    await sendingB
    expect(stop).toHaveBeenCalledOnce()
    one.resolve()
    await sendingA
    expect(stop).toHaveBeenCalledTimes(2)
    expect(stop.mock.calls[1]![0]).toBe(targetA)
  } finally {
    ids.mockRestore()
  }
})

it("retry consumes the original immutable mode, action and message after edits and tab switches (PS9)", async () => {
  const echo = scenarioEffects("echo")
  const submit = vi
    .fn(echo.submit)
    .mockRejectedValueOnce(new Error("lost acknowledgement"))
  const { store } = fixture({ submit, create: echo.create }, echo)
  const input = { id: "c0", steering: true, content: textContent("original") }
  await store.dispatch(sendDraft(input))
  const original = submit.mock.calls[0]![0]
  input.steering = false
  input.content[0] = { type: "text", text: "edited caller" }
  store.dispatch(setDraft({ id: "c0", draft: textContent("new draft") }))
  store.dispatch(openConversation())
  store.dispatch(setActive("c1"))
  await store.dispatch(
    controlConversation({
      id: "c0",
      control: { kind: "retry", target: original.target },
    }),
  )
  expect(submit).toHaveBeenCalledTimes(2)
  expect(submit.mock.calls[1]![0]).toBe(original)
  expect(original.mode).toBe("steering")
  expect(original.text).toBe("original")
  expect(store.getState().conversation.activeId).toBe("c1")
  expect(store.getState().conversation.conversations[0]!.draft).toEqual(
    textContent("new draft"),
  )
})

it.each([
  ["unknown", false, true],
  ["unknown", true, true],
  ["accepted", true, true],
  ["accepted", false, false],
] as const)(
  "retention: receipt=%s inflight=%s retained=%s",
  (receipt, inflight, retained) => {
    const prepared = prepareSubmission({
      conversationId: first,
      executionId: "execution",
      requestId: "request",
      mode: "queued",
      text: "original",
      attachments: [],
      files: [],
    })
    const operation = { prepared, stopRequested: false, inflight }
    const table = new PreparedOperationTable()
    table.set("c0", prepared.executionId, operation)

    table.prune([
      {
        id: "c0",
        turns: [{ from: "user", executionId: prepared.executionId, receipt }],
      },
    ])
    expect(table.get("c0", prepared.executionId)).toBe(retained ? operation : undefined)
  },
)

it.each(["conversation", "execution"] as const)(
  "a mismatched %s receipt remains unknown and retries the original value (PS10)",
  async (field) => {
    const echo = scenarioEffects("echo")
    const submit = vi.fn(echo.submit).mockImplementationOnce(async (prepared) => ({
      conversationId: field === "conversation" ? second : prepared.conversationId,
      executionId: field === "execution" ? "other" : prepared.executionId,
      disposition: "queued",
    }))
    const { store } = fixture({ submit, create: echo.create }, echo)
    await store.dispatch(sendDraft({ content: textContent("original") }))
    expect(store.getState().conversation.conversations[0]!.turns[0]).toMatchObject({
      receipt: "unknown",
    })
    const original = submit.mock.calls[0]![0]
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "retry", target: original.target },
      }),
    )
    expect(submit.mock.calls[1]![0]).toBe(original)
  },
)

it.each(["stop", "remove", "retry"] as const)(
  "%s rejects copied or differently routed targets before effects (PS11)",
  async (kind) => {
    const { store, stop, remove, submit } = fixture()
    await store.dispatch(sendDraft({ content: textContent("original") }))
    const captured = targetOf(store)
    store.dispatch(openConversation())
    store.dispatch(bindConversation({ id: "c1", serverId: second }))
    await store.dispatch(
      controlConversation({ id: "c1", control: { kind, target: captured } }),
    )
    await store.dispatch(
      controlConversation({ id: "c0", control: { kind, target: { ...captured } } }),
    )
    expect(stop).not.toHaveBeenCalled()
    expect(remove).not.toHaveBeenCalled()
    expect(submit).toHaveBeenCalledOnce()
  },
)

it("stop during retry admission reaches its captured target immediately and after admission (PS12)", async () => {
  const waiting = gate(),
    echo = scenarioEffects("echo")
  const submit = vi
    .fn(echo.submit)
    .mockRejectedValueOnce(new Error("lost acknowledgement"))
    .mockImplementationOnce(async (prepared) => {
      await waiting.promise
      return echo.submit(prepared)
    })
  const { store, stop } = fixture({ submit, create: echo.create }, echo)
  await store.dispatch(sendDraft({ content: textContent("original") }))
  const target = submit.mock.calls[0]![0].target
  const retrying = store.dispatch(
    controlConversation({ id: "c0", control: { kind: "retry", target } }),
  )
  await vi.waitFor(() => expect(submit).toHaveBeenCalledTimes(2))
  store.dispatch(submissionAccepted({ id: "c0", executionId: target.executionId }))
  const stopping = store.dispatch(stopGenerating())
  expect(stop).toHaveBeenCalledOnce()
  await stopping
  expect(store.getState().conversation.conversations[0]!.controlPending).toBe(true)
  waiting.resolve()
  await Promise.all([retrying, stopping])
  expect(stop).toHaveBeenCalledTimes(2)
  expect(stop.mock.calls.every((call) => call[0] === target)).toBe(true)
})

it("failed post-admission Stop acknowledgement does not rewrite known message admission (PS13)", async () => {
  const waiting = gate(),
    echo = scenarioEffects("echo")
  const submit = vi.fn(async (prepared: PreparedSubmission) => {
    await waiting.promise
    return echo.submit(prepared)
  })
  const stop = vi
    .fn<ConversationEffects["stop"]>()
    .mockResolvedValueOnce()
    .mockRejectedValueOnce(new Error("stop acknowledgement lost"))
  const { store } = fixture({ submit, stop, create: echo.create }, echo)
  const sending = store.dispatch(sendDraft({ content: textContent("original") }))
  await vi.waitFor(() => expect(submit).toHaveBeenCalledOnce())
  await store.dispatch(stopGenerating())
  waiting.resolve()
  const result = await sending
  expect(result.meta.requestStatus).toBe("fulfilled")
  expect(store.getState().conversation.conversations[0]!.turns[0]).toMatchObject({
    receipt: "delivered",
  })
  expect(store.getState().conversation.conversations[0]!.error).toBe(
    "stop acknowledgement lost",
  )
})

it("late admission repeats Stop only for the earlier captured operation while newer work exists (PS15)", async () => {
  const waiting = gate(),
    echo = scenarioEffects("echo")
  let next = 0
  const submit = vi.fn(async (prepared: PreparedSubmission) => {
    if (next++ === 0) await waiting.promise
    return echo.submit(prepared)
  })
  const { store, stop } = fixture({ submit, create: echo.create }, echo)
  const earlier = store.dispatch(sendDraft({ content: textContent("earlier") }))
  await vi.waitFor(() => expect(submit).toHaveBeenCalledOnce())
  await store.dispatch(stopGenerating())
  await store.dispatch(sendDraft({ content: textContent("newer") }))
  waiting.resolve()
  await earlier
  const original = submit.mock.calls[0]![0]
  expect(submit.mock.calls[1]![0].executionId).not.toBe(original.executionId)
  expect(stop.mock.calls.map((call) => call[0])).toEqual([
    original.target,
    original.target,
  ])
})

it("an immediate pre-admission Stop refusal retains intent for that later admission (PS17)", async () => {
  const waiting = gate(),
    echo = scenarioEffects("echo")
  const create = vi.fn(async (id: string) => {
    await waiting.promise
    return echo.create(id)
  })
  const stop = vi
    .fn<ConversationEffects["stop"]>()
    .mockRejectedValueOnce(new ControlFailedError("conversation-not-found", "refused"))
    .mockResolvedValueOnce()
  const { store, submit } = fixture({ create, stop }, echo)
  const sending = store.dispatch(sendDraft({ content: textContent("original") }))
  const stopped = await store.dispatch(stopGenerating())
  expect(stopped.meta.requestStatus).toBe("rejected")
  expect(submit).not.toHaveBeenCalled()
  const captured = stop.mock.calls[0]![0]
  waiting.resolve()
  const admitted = await sending
  expect(admitted.meta.requestStatus).toBe("fulfilled")
  expect(stop).toHaveBeenCalledTimes(2)
  expect(stop.mock.calls[1]![0]).toBe(captured)
})

it.each([false, true])(
  "retains retry intent before create even when tab closure prunes the table (PS22 closed=%s)",
  async (closed) => {
    const waiting = gate(),
      echo = scenarioEffects("echo")
    let creates = 0
    const create = vi.fn(async (id: string) => {
      if (creates++ === 1) await waiting.promise
      return echo.create(id)
    })
    const submit = vi.fn(echo.submit).mockRejectedValueOnce(new Error("unknown receipt"))
    const stop = vi
      .fn<ConversationEffects["stop"]>()
      .mockRejectedValueOnce(new ControlFailedError("conversation-not-found", "refused"))
      .mockResolvedValueOnce()
    const { store } = fixture({ create, submit, stop }, echo)
    await store.dispatch(sendDraft({ content: textContent("original") }))
    const prepared = submit.mock.calls[0]![0]
    const retryCommand: Control = { kind: "retry", target: prepared.target }
    const retrying = store.dispatch(
      controlConversation({ id: "c0", control: retryCommand }),
    )
    Object.assign(retryCommand, { kind: "close" })
    expect(create).toHaveBeenCalledTimes(2)
    await store.dispatch(stopGenerating())
    if (closed) store.dispatch(closeConversation("c0"))
    // Opening another operation invokes normal pruning of the closed tab's table.
    store.dispatch(openConversation())
    await store.dispatch(sendDraft({ content: textContent("new operation") }))
    waiting.resolve()
    await retrying
    expect(submit.mock.calls.at(-1)![0]).toBe(prepared)
    expect(stop.mock.calls.map((call) => call[0])).toEqual([
      prepared.target,
      prepared.target,
    ])
  },
)

it.each([
  "close",
  "remove",
  "answer",
  "cancel",
  "setApprovalMode",
  "reorder",
  "answerQuestion",
] as const)("captures %s command before create can suspend it (PS21)", async (kind) => {
  const waiting = gate(),
    echo = scenarioEffects("echo")
  const target = executionTargets({
    conversationId: first,
    messages: [],
    pending: [{ executionId: "original" }],
  })[0]!
  const permission = permissionTargets({
    conversationId: first,
    permissions: [
      { executionId: "original", permissionId: "review", options: [{ id: "allow" }] },
    ],
  })[0]!
  const create = vi.fn(async (id: string) => {
    await waiting.promise
    return echo.create(id)
  })
  const remove = vi.fn<ConversationEffects["remove"]>(async () => {})
  const answer = vi.fn<ConversationEffects["answer"]>(async () => {})
  const cancel = vi.fn<ConversationEffects["cancel"]>(async () => {})
  const setApprovalMode = vi.fn<ConversationEffects["setApprovalMode"]>(
    async (_id, mode) => mode,
  )
  const reorder = vi.fn<ConversationEffects["reorder"]>(async () => "applied")
  const answerQuestion = vi.fn<ConversationEffects["answerQuestion"]>(async () => {})
  const close = vi.fn<ConversationEffects["close"]>(async () => {})
  const { store } = fixture(
    { create, remove, answer, cancel, setApprovalMode, reorder, answerQuestion, close },
    echo,
  )
  const question = questionTarget()
  const choices = [{ key: "key", values: ["original"] }]
  const command: Control =
    kind === "close"
      ? { kind }
      : kind === "remove"
        ? { kind, target }
        : kind === "answer"
          ? { kind, target: permission, optionId: "allow" }
          : kind === "cancel"
            ? { kind, target: permission }
            : kind === "setApprovalMode"
              ? { kind, mode: "ask" }
              : kind === "reorder"
                ? { kind, executionIds: ["original"] }
                : {
                    kind,
                    target: question,
                    choices,
                  }
  const pending = store.dispatch(controlConversation({ id: "c0", control: command }))
  if (command.kind === "reorder") command.executionIds[0] = "changed"
  if (command.kind === "answerQuestion") choices[0]!.values[0] = "changed"
  // Replacing the discriminant after route checks must not select a new effect.
  Object.assign(command, {
    kind: kind === "close" ? "remove" : "close",
    target: executionTargets({
      conversationId: second,
      messages: [],
      pending: [{ executionId: "other" }],
    })[0]!,
    optionId: "changed",
    mode: "dontAsk",
  })
  waiting.resolve()
  await pending
  if (kind === "close") expect(close).toHaveBeenCalledExactlyOnceWith(first)
  else expect(close).not.toHaveBeenCalled()
  if (kind === "remove") expect(remove).toHaveBeenCalledExactlyOnceWith(target)
  if (kind === "answer")
    expect(answer).toHaveBeenCalledExactlyOnceWith(permission, "allow")
  if (kind === "cancel") expect(cancel).toHaveBeenCalledExactlyOnceWith(permission)
  if (kind === "setApprovalMode")
    expect(setApprovalMode).toHaveBeenCalledExactlyOnceWith(first, "ask")
  if (kind === "reorder")
    expect(reorder).toHaveBeenCalledExactlyOnceWith(first, ["original"])
  if (kind === "answerQuestion")
    expect(answerQuestion).toHaveBeenCalledExactlyOnceWith(question, [
      { key: "key", values: ["original"], ownWords: undefined },
    ])
})

it("serializes captured question DTOs without caller hooks after create awaits (PS23)", async () => {
  const waiting = gate(),
    echo = scenarioEffects("echo")
  const create = vi.fn(async (id: string) => {
    await waiting.promise
    return echo.create(id)
  })
  const valuesHook = vi.fn(() => ["substituted"])
  const recordHook = vi.fn(() => ({ key: "other", values: ["substituted"] }))
  const listHook = vi.fn(() => [{ key: "other", values: ["substituted"] }])
  const values = Object.assign(["original"], { toJSON: valuesHook })
  const choice = { key: "key", values, ownWords: "original words", toJSON: recordHook }
  const choices = Object.assign([choice], { toJSON: listHook })
  let serialized: string | undefined
  const answerQuestion = vi.fn<ConversationEffects["answerQuestion"]>(
    async (_target, captured) => {
      serialized = JSON.stringify(captured)
    },
  )
  const { store } = fixture({ create, answerQuestion }, echo)
  const pending = store.dispatch(
    controlConversation({
      id: "c0",
      control: {
        kind: "answerQuestion",
        target: questionTarget(),
        choices,
      },
    }),
  )
  choice.ownWords = "edited words"
  values[0] = "edited"
  waiting.resolve()
  const result = await pending
  expect(result.meta.requestStatus).toBe("fulfilled")
  expect(JSON.parse(serialized ?? "null")).toEqual([
    { key: "key", values: ["original"], ownWords: "original words" },
  ])
  expect(valuesHook).not.toHaveBeenCalled()
  expect(recordHook).not.toHaveBeenCalled()
  expect(listHook).not.toHaveBeenCalled()
})

it.each(["success", "create-refused", "admission-refused"] as const)(
  "retains a synchronous first-publication Stop through %s and closure/pruning (PS25–26)",
  async (outcome) => {
    for (const closed of [false, true])
      for (const stopRefused of [false, true]) {
        const waiting = gate(),
          echo = scenarioEffects("echo")
        let originalId: string | undefined
        const create = vi.fn(async (id: string) => {
          if (id === originalId) {
            await waiting.promise
            if (outcome === "create-refused")
              throw new SubmissionRefusedError("conversation-not-found")
          }
          return echo.create(id)
        })
        const submit = vi.fn(async (prepared: PreparedSubmission) => {
          if (prepared.text === "original" && outcome === "admission-refused")
            throw new SubmissionRefusedError("conversation-not-found")
          return echo.submit(prepared)
        })
        const stop = vi.fn<ConversationEffects["stop"]>(async () => {})
        if (stopRefused)
          stop.mockRejectedValueOnce(
            new ControlFailedError("conversation-not-found", "refused"),
          )
        const store = makeStore(
          createDependencies({ conversation: { ...echo, create, submit, stop } }),
        )
        expect(
          store.getState().conversation.conversations[0]!.serverConversationId,
        ).toBeUndefined()
        let observed = false
        let partialPublications = 0
        let stopping: Promise<unknown> = Promise.resolve()
        let replacement: Promise<unknown> = Promise.resolve()
        const unsubscribe = store.subscribe(() => {
          const current = store
            .getState()
            .conversation.conversations.find((tab) => tab.id === "c0")
          if (
            !observed &&
            current?.serverConversationId &&
            !current.turns.some((turn) => turn.from === "user")
          )
            partialPublications++
          const turn = current?.turns.find(
            (turn) => turn.from === "user" && turn.receipt === "sending",
          )
          if (observed || !turn) return
          observed = true
          originalId = current!.serverConversationId
          expect(current!.serverConversationId).toEqual(expect.any(String))
          stopping = store.dispatch(stopGenerating({ conversationId: "c0" }))
          if (closed) {
            store.dispatch(closeConversation("c0"))
            store.dispatch(openConversation())
            replacement = store.dispatch(
              sendDraft({ content: textContent("replacement") }),
            )
          }
        })
        const sending = store.dispatch(
          sendDraft({ id: "c0", content: textContent("original") }),
        )
        expect(observed).toBe(true)
        expect(partialPublications).toBe(0)
        expect(stop).toHaveBeenCalledOnce()
        const captured = stop.mock.calls[0]![0]
        await replacement
        waiting.resolve()
        const result = await sending
        await stopping
        unsubscribe()
        expect(result.meta.requestStatus).toBe(
          outcome === "success" ? "fulfilled" : "rejected",
        )
        expect(stop).toHaveBeenCalledTimes(outcome === "success" ? 2 : 1)
        if (outcome === "success") expect(stop.mock.calls[1]![0]).toBe(captured)
        const original = submit.mock.calls.find(
          ([prepared]) => prepared.text === "original",
        )?.[0]
        if (outcome === "create-refused") expect(original).toBeUndefined()
        else expect(original!.target).toBe(captured)
      }
  },
)

it("rejects an absent local tab before publishing or creating an operation (PS27)", async () => {
  const create = vi.fn(),
    submit = vi.fn(),
    stop = vi.fn()
  const { store } = fixture({ create, submit, stop })
  const before = store.getState().conversation
  const result = await store.dispatch(
    sendDraft({ id: "missing", content: textContent("original") }),
  )
  expect(result).toMatchObject({
    meta: { requestStatus: "rejected" },
    payload: { kind: "no-such-conversation" },
  })
  expect(store.getState().conversation).toBe(before)
  expect(create).not.toHaveBeenCalled()
  expect(submit).not.toHaveBeenCalled()
  expect(stop).not.toHaveBeenCalled()
})

it("rejects a copied prepared value before binding or publishing its turn (PS1)", () => {
  const store = makeStore(createDependencies({ conversation: scenarioEffects("echo") }))
  const before = store.getState().conversation
  const prepared = prepareSubmission({
    conversationId: first,
    executionId: "execution",
    requestId: "action",
    text: "original",
    mode: "queued",
  })
  expect(() =>
    store.dispatch(
      submissionStarted({
        conversationId: "c0",
        content: textContent("original"),
        prepared: { ...prepared },
      }),
    ),
  ).toThrow(TypeError)
  expect(store.getState().conversation).toBe(before)
})
