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
