import { expect, it, vi } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { scenarioEffects } from "../scenario/effects"
import type { ConversationView } from "../../application/view"
import { bindConversation, controlConversation, refreshConversation } from "./slice"

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
function view(ids: string[]): ConversationView {
  return {
    conversationId: "server",
    revision: ids.join(","),
    truncated: false,
    queueComplete: true,
    messages: [],
    permissions: [],
    tools: [],
    pending: ids.map((executionId) => ({
      executionId,
      text: executionId,
      attachments: [],
      files: [],
      mode: "queued",
    })),
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
    lifecycle: { phase: "attached" },
  }
}
for (const outcome of [
  "applied",
  "unchanged",
  "queue_changed",
  "priority_conflict",
] as const) {
  it(`refreshes authoritative order after ${outcome} without changing message identities`, async () => {
    const effects = scenarioEffects("echo")
    const serverOrder =
      outcome === "applied"
        ? ["b", "a"]
        : outcome === "queue_changed"
          ? ["a", "c"]
          : ["a", "b"]
    const read = vi
      .fn()
      .mockResolvedValueOnce(view(["a", "b"]))
      .mockResolvedValueOnce(view(serverOrder))
    const reorder = vi.fn(async () => outcome)
    const send = vi.fn(effects.send)
    const remove = vi.fn(effects.remove)
    const store = makeStore(
      createDependencies({ conversation: { ...effects, read, reorder, send, remove } }),
    )
    store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
    await store.dispatch(refreshConversation("c0"))
    await store.dispatch(
      controlConversation({
        id: "c0",
        control: { kind: "reorder", executionIds: ["b", "a"] },
      }),
    )
    const tab = store.getState().conversation.conversations[0]!
    expect(tab.remote?.pending.map((item) => item.executionId)).toEqual(serverOrder)
    expect(reorder).toHaveBeenCalledExactlyOnceWith("server", ["b", "a"])
    expect(send).not.toHaveBeenCalled()
    expect(remove).not.toHaveBeenCalled()
    expect(tab.controlPending).toBe(false)
    if (outcome === "queue_changed") expect(tab.error).toContain("queue changed")
    if (outcome === "priority_conflict") expect(tab.error).toContain("Steering")
  })
}
it("refreshes after a lost reorder acknowledgement without replaying the control", async () => {
  const effects = scenarioEffects("echo")
  const read = vi.fn().mockResolvedValue(view(["b", "a"]))
  const reorder = vi.fn(async () => {
    throw new Error("acknowledgement lost")
  })
  const store = makeStore(
    createDependencies({ conversation: { ...effects, read, reorder } }),
  )
  store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
  await store.dispatch(
    controlConversation({
      id: "c0",
      control: { kind: "reorder", executionIds: ["b", "a"] },
    }),
  )
  const tab = store.getState().conversation.conversations[0]!
  expect(tab.remote?.pending.map((item) => item.executionId)).toEqual(["b", "a"])
  expect(tab.error).toBe("acknowledgement lost")
  expect(reorder).toHaveBeenCalledTimes(1)
  expect(read).toHaveBeenCalledTimes(1)
  expect(tab.controlPending).toBe(false)
})
it("captures order before awaiting creation and rejects duplicate controls while busy", async () => {
  const effects = scenarioEffects("echo")
  const gate = deferred<{ conversationId: string }>()
  const create = vi.fn(() => gate.promise)
  const reorder = vi.fn(async () => "applied" as const)
  const store = makeStore(
    createDependencies({
      conversation: { ...effects, create, reorder, read: async () => view(["b", "a"]) },
    }),
  )
  store.dispatch(bindConversation({ id: "c0", serverId: "server" }))
  const ids = ["b", "a"]
  const first = store.dispatch(
    controlConversation({ id: "c0", control: { kind: "reorder", executionIds: ids } }),
  )
  ids.reverse()
  await store.dispatch(
    controlConversation({
      id: "c0",
      control: { kind: "reorder", executionIds: ["a", "b"] },
    }),
  )
  expect(create).toHaveBeenCalledTimes(1)
  expect(reorder).not.toHaveBeenCalled()
  gate.resolve({ conversationId: "server" })
  await first
  expect(reorder).toHaveBeenCalledExactlyOnceWith("server", ["b", "a"])
})
