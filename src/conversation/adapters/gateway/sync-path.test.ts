import { NessaRpcError } from "@nessa/client"
import { expect, it } from "vitest"

import { createChangeFollower, type FollowSocket } from "./sync-path"

function socket() {
  const changed = new Set<(payload: { watchId: string; reason?: string }) => void>()
  const ended = new Set<(payload: { watchId: string; reason?: string }) => void>()
  const states = new Set<(state: { status: string }) => void>()
  const calls: string[] = []
  let catalogueFails = 0
  let recordFails = 0
  const api: FollowSocket = {
    watches: {
      catalogue: async () => {
        calls.push("catalogue")
        if (catalogueFails > 0) {
          catalogueFails -= 1
          throw new NessaRpcError("watch_capacity", "watch_capacity")
        }
        return { watchId: "catalogue-watch" }
      },
      records: async ({ conversationId }) => {
        calls.push(`records:${conversationId}`)
        if (recordFails > 0) {
          recordFails -= 1
          throw new NessaRpcError("watch_capacity", "watch_capacity")
        }
        return { watchId: `record-${conversationId}` }
      },
      unwatch: async (watchId) => {
        calls.push(`unwatch:${watchId}`)
        return { watchId }
      },
    },
    on: (event, handler) => {
      const set = event === "conversation.changed" ? changed : ended
      set.add(handler)
      return () => set.delete(handler)
    },
    onConnectionStateChange: (handler) => {
      states.add(handler)
      return () => states.delete(handler)
    },
    close: () => {
      calls.push("close")
    },
  }
  return {
    api,
    calls,
    emit(watchId: string) {
      for (const handler of changed) handler({ watchId })
    },
    end(watchId: string) {
      for (const handler of ended) handler({ watchId, reason: "closed" })
    },
    closeConnection() {
      for (const handler of states) handler({ status: "closed" })
    },
    state(status: string) {
      for (const handler of states) handler({ status })
    },
    failCatalogue(times: number) {
      catalogueFails = times
    },
    failRecord(times: number) {
      recordFails = times
    },
  }
}

it("signals a list change and does not read", async () => {
  const follow = socket()
  const events: string[] = []
  const follower = createChangeFollower({
    openConnection: async () => follow.api,
    recordTargets: () => [],
    onListChanged: () => events.push("list"),
    onConversationChanged: (id) => events.push(id),
    onRecordHeld: () => {},
    onWatching: () => {},
    onFallback: (reason) => events.push(reason),
  })
  await expect(follower.start()).resolves.toBe("sync")
  follow.emit("catalogue-watch")
  expect(events).toEqual(["list"])
  expect(follow.calls).toEqual(["catalogue"])
  follower.stop()
})

it("signals the watched chat and keeps the list when that watch ends", async () => {
  const follow = socket()
  const events: string[] = []
  let held: string | undefined
  const follower = createChangeFollower({
    openConnection: async () => follow.api,
    recordTargets: () => ["chat-a"],
    onListChanged: () => events.push("list"),
    onConversationChanged: (id) => events.push(`changed:${id}`),
    onRecordHeld: (id) => {
      held = id
    },
    onWatching: () => {},
    onFallback: (reason) => events.push(reason),
  })
  await follower.start()
  expect(held).toBe("chat-a")
  follow.emit("record-chat-a")
  follow.end("record-chat-a")
  expect(events).toEqual(["changed:chat-a"])
  expect(held).toBeUndefined()
  expect(follower.recordTarget()).toBeUndefined()
  follower.stop()
})

it("falls back when the catalogue watch ends or the socket closes", async () => {
  const ended = socket()
  const reasons: string[] = []
  const follower = createChangeFollower({
    openConnection: async () => ended.api,
    recordTargets: () => [],
    onListChanged: () => {},
    onConversationChanged: () => {},
    onRecordHeld: () => {},
    onWatching: () => {},
    onFallback: (reason) => reasons.push(reason),
  })
  await follower.start()
  ended.end("catalogue-watch")
  expect(reasons).toEqual(["watch-ended"])

  const closed = socket()
  const followerClosed = createChangeFollower({
    openConnection: async () => closed.api,
    recordTargets: () => [],
    onListChanged: () => {},
    onConversationChanged: () => {},
    onRecordHeld: () => {},
    onWatching: () => {},
    onFallback: (reason) => reasons.push(reason),
  })
  await followerClosed.start()
  closed.closeConnection()
  expect(reasons).toEqual(["watch-ended", "watch-ended"])
})

it("retries a full watch and refuses the window on an access error", async () => {
  const follow = socket()
  follow.failCatalogue(2)
  const waits: number[] = []
  const follower = createChangeFollower({
    openConnection: async () => follow.api,
    recordTargets: () => [],
    onListChanged: () => {},
    onConversationChanged: () => {},
    onRecordHeld: () => {},
    onWatching: () => {},
    onFallback: () => {},
    wait: async (ms) => {
      waits.push(ms)
    },
  })
  await expect(follower.start()).resolves.toBe("sync")
  expect(follow.calls.filter((call) => call === "catalogue")).toHaveLength(3)
  expect(waits).toEqual([50, 200])
  follower.stop()

  const refused = socket()
  const reasons: string[] = []
  refused.api.watches.catalogue = async () => {
    throw new NessaRpcError("forbidden", "forbidden")
  }
  const denied = createChangeFollower({
    openConnection: async () => refused.api,
    recordTargets: () => [],
    onListChanged: () => {},
    onConversationChanged: () => {},
    onRecordHeld: () => {},
    onWatching: () => {},
    onFallback: (reason) => reasons.push(reason),
  })
  await expect(denied.start()).resolves.toBe("refused")
  expect(reasons).toEqual([])
})

it("moves the one record watch when the target changes", async () => {
  const follow = socket()
  let target = "chat-a"
  const held: Array<string | undefined> = []
  const follower = createChangeFollower({
    openConnection: async () => follow.api,
    recordTargets: () => [target],
    onListChanged: () => {},
    onConversationChanged: () => {},
    onRecordHeld: (id) => held.push(id),
    onWatching: () => {},
    onFallback: () => {},
  })
  await follower.start()
  target = "chat-b"
  follower.retarget()
  for (let i = 0; i < 10; i++) await Promise.resolve()
  expect(held).toEqual(["chat-a", "chat-b"])
  expect(follow.calls).toContain("unwatch:record-chat-a")
  expect(follow.calls).toContain("records:chat-b")
  follower.stop()
})

it("reports in-process time from a ping to the callback", async () => {
  const follow = socket()
  let at = 0
  const follower = createChangeFollower({
    openConnection: async () => follow.api,
    recordTargets: () => [],
    onListChanged: () => {
      at = performance.now()
    },
    onConversationChanged: () => {},
    onRecordHeld: () => {},
    onWatching: () => {},
    onFallback: () => {},
  })
  await follower.start()
  const started = performance.now()
  follow.emit("catalogue-watch")
  const elapsed = at - started
  // This is the follower's own callback, on a fake socket. It is not a
  // gateway round trip and not commit-to-screen.
  expect(elapsed).toBeGreaterThanOrEqual(0)
  expect(elapsed).toBeLessThan(50)
  follower.stop()
})

it("suspends while the watch reconnects and registers again when it connects", async () => {
  const follow = socket()
  const watching: boolean[] = []
  const reasons: string[] = []
  const follower = createChangeFollower({
    openConnection: async () => follow.api,
    recordTargets: () => ["chat-a"],
    onListChanged: () => {},
    onConversationChanged: () => {},
    onRecordHeld: () => {},
    onWatching: (held) => watching.push(held),
    onFallback: (reason) => reasons.push(reason),
  })
  await expect(follower.start()).resolves.toBe("sync")
  follow.state("reconnecting")
  expect(follower.recordTarget()).toBeUndefined()
  expect(watching).toEqual([false])
  expect(reasons).toEqual([])
  expect(follow.calls).not.toContain("close")
  follow.state("connected")
  for (let i = 0; i < 10; i++) await Promise.resolve()
  expect(watching).toEqual([false, true])
  expect(follow.calls.filter((call) => call === "catalogue")).toHaveLength(2)
  expect(follow.calls).toContain("records:chat-a")
  expect(reasons).toEqual([])
  follower.stop()
})

it("drops a deleted record watch and keeps the catalogue connection", async () => {
  const follow = socket()
  let held: string | undefined = "pending"
  const reasons: string[] = []
  let target = "chat-a"
  const follower = createChangeFollower({
    openConnection: async () => follow.api,
    recordTargets: () => [target],
    onListChanged: () => {},
    onConversationChanged: () => {},
    onRecordHeld: (id) => {
      held = id
    },
    onWatching: () => {},
    onFallback: (reason) => reasons.push(reason),
  })
  await follower.start()
  follow.api.watches.records = async () => {
    throw new NessaRpcError("wrong_owner", "wrong_owner")
  }
  follow.end("record-chat-a")
  target = "chat-a"
  follower.retarget()
  for (let i = 0; i < 10; i++) await Promise.resolve()
  expect(reasons).toEqual([])
  expect(held).toBeUndefined()
  expect(follow.calls).not.toContain("close")
  target = "chat-b"
  follow.api.watches.records = async ({ conversationId }) => {
    follow.calls.push(`records:${conversationId}`)
    return { watchId: `record-${conversationId}` }
  }
  follower.retarget()
  for (let i = 0; i < 10; i++) await Promise.resolve()
  expect(held).toBe("chat-b")
  expect(reasons).toEqual([])
  follower.stop()
})

it("returns retry when the catalogue watch stays at capacity", async () => {
  const follow = socket()
  follow.failCatalogue(8)
  const waits: number[] = []
  const follower = createChangeFollower({
    openConnection: async () => follow.api,
    recordTargets: () => [],
    onListChanged: () => {},
    onConversationChanged: () => {},
    onRecordHeld: () => {},
    onWatching: () => {},
    onFallback: () => {},
    wait: async (ms) => {
      waits.push(ms)
    },
  })
  await expect(follower.start()).resolves.toBe("retry")
  expect(waits).toEqual([50, 200, 500])
  expect(follow.calls.filter((call) => call === "catalogue")).toHaveLength(4)
})
