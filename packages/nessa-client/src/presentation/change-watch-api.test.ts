import { describe, expect, it, vi } from "vitest"
import { createChangeWatchApi } from "./change-watch-api.js"
import { bounds, passiveReadTiming, ProductMethod } from "../generated/product.js"
import type { RpcRequester } from "../application/session-port.js"
const conversationId = "00000000-0000-4000-8000-000000000001"
const watchId = `${conversationId}-1`
function fixture(response: unknown = { watchId }) {
  const session = { request: vi.fn().mockResolvedValue(response) } satisfies RpcRequester
  return { watches: createChangeWatchApi(session), sent: session.request }
}

describe("connection-local watch API", () => {
  it("shares the existing dispatcher and published deadline floor without a head read", async () => {
    const { watches, sent } = fixture()
    const params = { conversationId, receiverId: "receiver", accessEpoch: "3" }
    await expect(watches.records(params)).resolves.toEqual({ watchId })
    expect(sent).toHaveBeenCalledExactlyOnceWith(
      ProductMethod.ConversationWatchRecords,
      params,
      { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
    )
  })
  it("refuses malformed receiver/epoch before any send", async () => {
    const { watches, sent } = fixture()
    await expect(
      watches.catalogue({ receiverId: "receiver", accessEpoch: "0" }),
    ).rejects.toThrow("Invalid watch receiver or epoch")
    await expect(
      watches.records({
        conversationId: "bad",
        receiverId: "receiver",
        accessEpoch: "1",
      }),
    ).rejects.toThrow("Invalid conversation ID")
    expect(sent).not.toHaveBeenCalled()
  })
  it("refuses invalid binding on either target kind before dispatch", async () => {
    const oversized = "😀".repeat(Math.floor(bounds.maxSyncIdBytes / 4) + 1)
    for (const params of [
      { receiverId: oversized, accessEpoch: "3" },
      { receiverId: "receiver", accessEpoch: "0" },
    ]) {
      const { watches, sent } = fixture()
      await expect(watches.records({ conversationId, ...params })).rejects.toThrow(
        "Invalid watch receiver or epoch",
      )
      await expect(watches.catalogue(params)).rejects.toThrow(
        "Invalid watch receiver or epoch",
      )
      expect(sent).not.toHaveBeenCalled()
    }
    const { watches, sent } = fixture({ watchId: `${conversationId}-2` })
    await expect(
      watches.catalogue({ receiverId: "receiver", accessEpoch: "3" }),
    ).resolves.toEqual({ watchId: `${conversationId}-2` })
    expect(sent).toHaveBeenCalledExactlyOnceWith(
      ProductMethod.ConversationWatchCatalogue,
      { receiverId: "receiver", accessEpoch: "3" },
      { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
    )
  })
  it("refuses malformed unwatch identity before dispatch", async () => {
    const { watches, sent } = fixture()
    for (const invalid of ["invalid-1", `${conversationId}-0`, `${conversationId}-01`]) {
      await expect(watches.unwatch(invalid)).rejects.toThrow("Invalid change watch ID")
    }
    expect(sent).not.toHaveBeenCalled()
  })
  it("rejects inherited identity with an equal number of own wrong fields", async () => {
    const inherited = Object.assign(Object.create({ watchId }), { other: true })
    const { watches } = fixture(inherited)
    await expect(
      watches.catalogue({ receiverId: "receiver", accessEpoch: "3" }),
    ).rejects.toThrow("Invalid change watch response")
  })
  it("does not let a foreign echoed ID confirm removal", async () => {
    const { watches } = fixture({ watchId: `${conversationId}-2` })
    await expect(watches.unwatch(watchId)).rejects.toThrow(
      "Mismatched change watch removal",
    )
  })
  it("preserves a refusal without retry or registration resurrection", async () => {
    const { watches, sent } = fixture()
    const error = new Error("watch_duplicate")
    sent.mockRejectedValue(error)
    await expect(
      watches.catalogue({ receiverId: "receiver", accessEpoch: "3" }),
    ).rejects.toBe(error)
    expect(sent).toHaveBeenCalledTimes(1)
  })
})
