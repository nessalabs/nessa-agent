// @vitest-environment jsdom
/** Preference commands complete after the authoritative subscription, never the ACK alone. */
import {
  NessaRpcError,
  type ViewFrame,
  type ViewSubscriptionHandlers,
} from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import type { WorkspaceUpdate } from "../../application/ports"
import { deferred, fakeGateway, view } from "./fake-gateway"
import { gatewaySource, type GatewayClock } from "./gateway-source"

const clock: GatewayClock = { now: () => 1_000_000, after: () => () => {} }
const flush = async () => {
  for (let i = 0; i < 50; i++) await Promise.resolve()
}

async function ready(beforeFollow?: (gateway: ReturnType<typeof fakeGateway>) => void) {
  const gateway = fakeGateway()
  beforeFollow?.(gateway)
  gateway.views.set("a", view("a", { usageLimitAutoContinue: false }))
  gateway.views.set("b", view("b", { usageLimitAutoContinue: false }))
  const source = gatewaySource({ connect: async () => gateway.client, clock })
  const updates: WorkspaceUpdate[] = []
  source.subscribe((update) => updates.push(update))
  await source.transcript("a")
  await source.transcript("b")
  const preferences = () =>
    updates.flatMap((update) =>
      update.kind === "transcript"
        ? [[update.transcript.sessionId, update.transcript.usageLimitAutoContinue]]
        : [],
    )
  return { gateway, source, preferences }
}

describe("usage-limit preference publication", () => {
  it("keeps the command pending after the RPC until every open conversation has a fresh frame", async () => {
    const { gateway, source, preferences } = await ready()
    const frame = deferred()
    gateway.once("subscribe", async (normal) => {
      await frame.promise
      return normal()
    })
    let completed = false
    const command = source.setUsageLimitAutoContinue(true, "person").then(() => {
      completed = true
    })
    await flush()
    expect(gateway.count("setUsageLimitAutoContinue")).toBe(1)
    expect(completed).toBe(false)
    expect(preferences()).not.toContainEqual(["a", true])
    frame.resolve()
    await command
    expect(preferences()).toContainEqual(["a", true])
    expect(preferences()).toContainEqual(["b", true])
    expect(
      gateway.calls
        .filter((call) => call.method === "subscribe")
        .slice(-2)
        .map((call) => call.args[1]),
    ).toEqual([{}, {}])
    source.dispose()
  })

  it("accepts the fresh external winner instead of projecting the requested value", async () => {
    const { gateway, source, preferences } = await ready()
    gateway.once("setUsageLimitAutoContinue", async () => ({
      requestId: "receipt-before-external-change",
      enabled: true,
    }))
    await source.setUsageLimitAutoContinue(true, "person")
    expect(preferences()).not.toContainEqual(["a", true])
    expect((await source.transcript("a")).usageLimitAutoContinue).toBe(false)
    expect(gateway.count("subscribe")).toBe(4)
    source.dispose()
  })

  it("waits for a live subscription's close ACK before reopening that target", async () => {
    const { gateway, source } = await ready()
    const closed = deferred()
    gateway.once("unsubscribe", async () => closed.promise)
    const command = source.setUsageLimitAutoContinue(true, "person")
    await flush()
    expect(
      gateway.calls.filter((call) => call.method === "subscribe" && call.args[0] === "a"),
    ).toHaveLength(1)
    closed.resolve()
    await command
    source.dispose()
  })

  it("rejects when the old live subscription cannot be closed", async () => {
    const { gateway, source } = await ready()
    gateway.once("unsubscribe", async () => {
      throw new NessaRpcError("storage_unavailable", "Close refused")
    })
    await expect(source.setUsageLimitAutoContinue(true, "person")).rejects.toMatchObject({
      reason: "unavailable",
    })
    expect(
      gateway.calls.filter((call) => call.method === "subscribe" && call.args[0] === "a"),
    ).toHaveLength(1)
    source.dispose()
  })

  it("coalesces an earlier command's remaining waiters onto a newer refresh", async () => {
    const { gateway, source, preferences } = await ready()
    const oldFrame = deferred()
    gateway.once("subscribe", async (normal) => normal())
    gateway.once("subscribe", async (normal) => {
      await oldFrame.promise
      return normal()
    })
    let firstCompleted = false
    const first = source.setUsageLimitAutoContinue(true, "person").then(() => {
      firstCompleted = true
    })
    await flush()
    expect(preferences()).toContainEqual(["a", true])
    expect(firstCompleted).toBe(false)
    const second = source.setUsageLimitAutoContinue(false, "person")
    await flush()
    expect(firstCompleted).toBe(false)
    oldFrame.resolve()
    await Promise.all([first, second])
    expect((await source.transcript("a")).usageLimitAutoContinue).toBe(false)
    expect((await source.transcript("b")).usageLimitAutoContinue).toBe(false)
    source.dispose()
  })

  it("ignores a late frame from the pre-command subscription", async () => {
    const captured: { handlers: ViewSubscriptionHandlers; frame?: ViewFrame }[] = []
    const { gateway, source, preferences } = await ready((gateway) => {
      const subscribe = gateway.client.subscriptions.view
      vi.spyOn(gateway.client.subscriptions, "view").mockImplementation(
        (id, handlers, options) => {
          const opening = { handlers, frame: undefined as ViewFrame | undefined }
          captured.push(opening)
          return subscribe(
            id,
            {
              ...handlers,
              view(frame) {
                opening.frame = frame
                handlers.view(frame)
              },
            },
            options,
          )
        },
      )
    })
    const old = captured[0]
    expect(old.frame).toBeDefined()
    const fresh = deferred()
    gateway.once("subscribe", async (normal) => {
      await fresh.promise
      return normal()
    })
    let completed = false
    const command = source.setUsageLimitAutoContinue(true, "person").then(() => {
      completed = true
    })
    await flush()
    old.handlers.view({
      ...old.frame!,
      view: view("a", { usageLimitAutoContinue: true }),
    })
    await flush()
    expect(preferences()).not.toContainEqual(["a", true])
    expect(completed).toBe(false)
    fresh.resolve()
    await command
    expect(preferences()).toContainEqual(["a", true])
    source.dispose()
  })

  it("rejects a committed preference whose fresh subscription cannot be read", async () => {
    const { gateway, source } = await ready()
    gateway.once("subscribe", async () => {
      throw new NessaRpcError("storage_unavailable", "No frame")
    })
    await expect(source.setUsageLimitAutoContinue(true, "person")).rejects.toMatchObject({
      reason: "unavailable",
    })
    source.dispose()
  })

  it("rejects when disposed while its post-ACK frame is still pending", async () => {
    const { gateway, source } = await ready()
    const frame = deferred()
    gateway.once("subscribe", async (normal) => {
      await frame.promise
      return normal()
    })
    const command = source.setUsageLimitAutoContinue(true, "person")
    const rejected = expect(command).rejects.toMatchObject({ reason: "unavailable" })
    await flush()
    source.dispose()
    frame.resolve()
    await rejected
  })
})
