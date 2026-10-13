import { executionTargets, type PreparedSubmission } from "@nessa/client"
import { expect, it, vi } from "vitest"
import { createDependencies } from "../../../composition/dependencies"
import { makeStore } from "../../../store"
import { scenarioEffects } from "../scenario/effects"
import { textContent } from "../../model"
import { ControlFailedError, type ConversationEffects } from "../../application/ports"
import {
  bindConversation,
  closeConversation,
  controlConversation,
  openConversation,
  sendDraft,
  setActive,
  setDraft,
  stopGenerating,
} from "./slice"

const first = "00000000-0000-4000-8000-000000000001"
const second = "00000000-0000-4000-8000-000000000002"
const sameExecution = "00000000-0000-4000-8000-000000000003"
function gate() {
  let resolve!: () => void
  const promise = new Promise<void>((done) => {
    resolve = done
  })
  return { promise, resolve }
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
