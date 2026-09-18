import type { NessaClient, ConversationView } from "@nessa/client"
import { expect, it, vi } from "vitest"
import { gatewayEffects } from "./effects"
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
it("joins concurrent creation and forwards exact stable submission IDs", async () => {
  const gate = deferred<{ conversationId: string }>()
  const create = vi.fn(() => gate.promise)
  const send = vi.fn(async () => ({
    executionId: "execution",
    requestId: "action",
    disposition: "queued" as const,
  }))
  const effects = gatewayEffects(
    () => ({ conversation: { create, send } }) as unknown as NessaClient,
  )
  const first = effects.create("server")
  const second = effects.create("server")
  // The agent setup chose is asked for before the creation goes out, so the
  // call lands a turn later; joining it does not wait for that.
  expect(first).toBe(second)
  await Promise.resolve()
  expect(create).toHaveBeenCalledOnce()
  gate.resolve({ conversationId: "server" })
  await Promise.all([first, second])
  await effects.send({
    conversationId: "server",
    executionId: "execution",
    actionId: "action",
    text: "exact",
  })
  expect(send).toHaveBeenCalledWith("server", "exact", {
    executionId: "execution",
    requestId: "action",
  })
})
it("serializes opaque-revision reads, including overlapping manual refreshes", async () => {
  const first = deferred<ConversationView>()
  const read = vi.fn(() => first.promise)
  const effects = gatewayEffects(
    () => ({ conversation: { read } }) as unknown as NessaClient,
  )
  const pending = effects.read("server")
  const following = effects.read("server")
  await Promise.resolve()
  await Promise.resolve()
  expect(read).toHaveBeenCalledOnce()
  first.resolve({ conversationId: "server" } as ConversationView)
  await Promise.all([pending, following])
  expect(read).toHaveBeenCalledTimes(2)
})

it("asks the host again after a failed answer instead of keeping the failure", async () => {
  // Every conversation is created through this, so a remembered rejection is
  // not one lost answer — it is a panel that can no longer start, send, close
  // or answer a permission until it is restarted.
  const create = vi.fn(async ({ conversationId }: { conversationId: string }) => ({
    conversationId,
  }))
  let asked = 0
  const chosenAgent = async () => {
    asked += 1
    if (asked === 1) throw new Error("the host module would not load")
    return "codex"
  }
  const effects = gatewayEffects(
    () => ({ conversation: { create } }) as unknown as NessaClient,
    chosenAgent,
  )
  await expect(effects.create("server")).rejects.toThrow("the host module would not load")
  expect(create).not.toHaveBeenCalled()
  await effects.create("server")
  expect(asked).toBe(2)
  expect(create).toHaveBeenCalledWith({ conversationId: "server", agent: "codex" })
})

it("sends the creation over the session of the moment, not the one checked first", async () => {
  // Asking the host is a round trip, and a session retired inside it must not
  // be the one this create goes out on.
  const retired = vi.fn()
  const live = vi.fn(async ({ conversationId }: { conversationId: string }) => ({
    conversationId,
  }))
  let current = retired
  const effects = gatewayEffects(
    () => ({ conversation: { create: current } }) as unknown as NessaClient,
    async () => {
      current = live
      return undefined
    },
  )
  await effects.create("server")
  expect(retired).not.toHaveBeenCalled()
  expect(live).toHaveBeenCalledOnce()
})
