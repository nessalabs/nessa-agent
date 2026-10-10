import { expect, it, vi } from "vitest"
import { createDependencies } from "../../../composition/dependencies"
import { makeStore } from "../../../store"
import type { ConversationListFollower } from "../../application/ports"
import type { ConversationListing } from "../../application/view"
import { scenarioEffects } from "../scenario/effects"
import {
  archiveConversation,
  deleteConversation,
  followConversations,
  listConversations,
} from "./history"

const row = (id: string, archived = false) => ({
  conversationId: id,
  title: id,
  preview: id,
  updatedAtMs: 1,
  running: false,
  archived,
})
const listing = (
  ids: string[],
  archived = false,
  complete = true,
): ConversationListing => ({
  conversations: ids.map((id) => row(id, archived)),
  complete,
})
function fixture(overrides: Partial<ReturnType<typeof scenarioEffects>> = {}) {
  const opens: Array<{
    archived: boolean
    follower: ConversationListFollower
    stop: ReturnType<typeof vi.fn>
  }> = []
  const effects = {
    ...scenarioEffects("echo"),
    ...overrides,
    followList(archived: boolean, follower: ConversationListFollower) {
      const stop = vi.fn()
      opens.push({ archived, follower, stop })
      return stop
    },
  }
  const store = makeStore(createDependencies({ conversation: effects }))
  const state = () => store.getState().conversationHistory
  const stop = store.dispatch(followConversations())
  const frame = (index: number, ids: string[], complete = true) =>
    opens[index]!.follower.list(listing(ids, opens[index]!.archived, complete))
  return { store, opens, state, stop, frame }
}

it.each([0, 1])(
  "PL1: publishes no half-list when initial half %i arrives first",
  (first) => {
    const f = fixture()
    expect(f.opens.map((x) => x.archived)).toEqual([false, true])
    f.frame(first, first === 0 ? ["active"] : ["archived"])
    expect(f.state().rows).toBeNull()
    f.frame(1 - first, first === 0 ? ["archived"] : ["active"])
    expect(f.state().rows?.map((x) => x.conversationId)).toEqual(["active"])
    expect(f.state().archivedIds).toEqual(["archived"])
    f.stop()
  },
)

it("PL2: follows external catalogue changes without guessing archive or deletion from omission", () => {
  const f = fixture()
  f.frame(0, ["active"])
  f.frame(1, ["archived"])
  f.frame(0, ["new"])
  expect(f.state().archivedIds).toEqual(["archived"])
  expect(f.state().deletedIds).toEqual([])
  f.frame(1, ["archived", "active"])
  expect(f.state().archivedIds).toEqual(["archived", "active"])
  f.frame(1, [])
  expect(f.state().rows?.map((x) => x.conversationId)).toEqual(["new"])
  expect(f.state().archivedIds).toEqual([])
  f.stop()
})

it("PL3: retains a failed half's stale notice through the other half's successful frames", () => {
  const f = fixture()
  f.opens[0]!.follower.failed("unavailable")
  f.frame(1, ["archived"])
  expect(f.state().rows).toBeNull()
  expect(f.state().failure).toBe("unavailable")
  f.frame(0, ["active"])
  expect(f.state().failure).toBeNull()
  f.opens[0]!.follower.failed("state-unreadable")
  f.frame(1, ["archived", "more"])
  expect(f.state().failure).toBe("state-unreadable")
  expect(f.state().rows?.[0]?.conversationId).toBe("active")
  f.frame(0, ["new"])
  expect(f.state().failure).toBeNull()
  f.stop()
})

it("PL4, PL7: stops the old pair and ignores its callbacks and cleanup after replacement", () => {
  const f = fixture()
  f.frame(0, ["old"])
  f.frame(1, ["old-archive"])
  const stopNew = f.store.dispatch(followConversations())
  expect(f.opens[0]!.stop).toHaveBeenCalledOnce()
  expect(f.opens[1]!.stop).toHaveBeenCalledOnce()
  f.frame(2, ["current"])
  f.frame(3, [])
  f.frame(0, ["late"])
  f.opens[1]!.follower.failed("unavailable")
  f.stop()
  expect(f.state().rows?.[0]?.conversationId).toBe("current")
  expect(f.state().following).toBe(true)
  expect(f.state().failure).toBeNull()
  expect(f.opens[2]!.stop).not.toHaveBeenCalled()
  stopNew()
  expect(f.opens[2]!.stop).toHaveBeenCalledOnce()
  expect(f.opens[3]!.stop).toHaveBeenCalledOnce()
  f.frame(2, ["after-stop"])
  expect(f.state().rows?.[0]?.conversationId).toBe("current")
  expect(f.state().following).toBe(false)
})

it("PL5: a late one-shot cannot overwrite followed facts; reads are allowed after unmount", async () => {
  let answer!: (value: ConversationListing) => void
  const active = new Promise<ConversationListing>((resolve) => {
    answer = resolve
  })
  const effects = scenarioEffects("echo")
  const opens: ConversationListFollower[] = []
  const list = vi.fn(async (archived: boolean) => (archived ? listing([], true) : active))
  const store = makeStore(
    createDependencies({
      conversation: {
        ...effects,
        list,
        followList(_archived, follower) {
          opens.push(follower)
          return () => {}
        },
      },
    }),
  )
  const old = store.dispatch(listConversations())
  const stop = store.dispatch(followConversations())
  opens[0]!.list(listing(["new"]))
  opens[1]!.list(listing([], true))
  answer(listing(["old"]))
  await old
  expect(store.getState().conversationHistory.rows?.[0]?.conversationId).toBe("new")
  stop()
  await store.dispatch(listConversations())
  expect(store.getState().conversationHistory.rows?.[0]?.conversationId).toBe("old")
})

it.each([false, true])(
  "PL6: archive result lost=%s restarts the same UI-owned pair and fences older frames",
  async (lost) => {
    const f = fixture({
      archive: async () => {
        if (lost) throw new Error("lost acknowledgement")
        return true
      },
    })
    f.frame(0, ["active"])
    f.frame(1, [])
    const before = f.state().requestId
    await f.store.dispatch(
      archiveConversation({
        serverConversationId: "active",
        archived: true,
        title: "Active",
      }),
    )
    expect(f.opens).toHaveLength(4)
    expect(f.state().requestId).not.toBe(before)
    if (!lost) expect(f.state().archivedIds).toContain("active")
    f.frame(0, ["stale"])
    f.frame(1, [])
    expect(f.state().rows?.map((row) => row.conversationId)).toEqual(
      lost ? ["active"] : [],
    )
    f.frame(2, [])
    f.frame(3, ["active"])
    expect(f.state().archivedIds).toEqual(["active"])
    f.stop()
    expect(f.opens.map((x) => x.stop.mock.calls.length)).toEqual([1, 1, 1, 1])
    expect(f.state().following).toBe(false)
  },
)

it("PL9: incomplete archived evidence preserves known identities until an explicit active row replaces them", () => {
  const f = fixture()
  f.frame(0, ["active"])
  f.frame(1, ["archived"])
  f.frame(1, [], false)
  expect(f.state().archivedIds).toEqual(["archived"])
  expect(f.state().complete).toBe(false)
  f.frame(0, ["active", "archived"])
  expect(f.state().archivedIds).toEqual([])
  f.frame(1, ["archived"], false)
  expect(f.state().archivedIds).toEqual(["archived"])
  f.stop()
})

it("PL10: replacement list frames cannot erase local deletion evidence", async () => {
  const f = fixture({ delete: async () => {} })
  f.frame(0, ["active"])
  f.frame(1, [])
  await f.store.dispatch(
    deleteConversation({ serverConversationId: "active", title: "Active" }),
  )
  f.frame(2, ["active"])
  f.frame(3, [])
  expect(f.state().deletedIds).toEqual(["active"])
  f.stop()
})

it("PL5: a one-shot started while following cannot displace it with a late failure", async () => {
  let fail!: (error: unknown) => void
  const held = new Promise<ConversationListing>((_resolve, reject) => {
    fail = reject
  })
  const f = fixture({ list: async () => held })
  f.frame(0, ["current"])
  f.frame(1, [])
  const asking = f.store.dispatch(listConversations())
  fail(new Error("old read failed"))
  await asking
  expect(f.state().rows?.[0]?.conversationId).toBe("current")
  expect(f.state().failure).toBeNull()
  expect(f.state().following).toBe(true)
  f.frame(0, ["next"])
  expect(f.state().rows?.[0]?.conversationId).toBe("next")
  f.stop()
})

it("PL6: undo restarts both follows and keeps the original cleanup owner", async () => {
  const f = fixture({ archive: async () => true })
  f.frame(0, [])
  f.frame(1, ["archived"])
  await f.store.dispatch(
    archiveConversation({
      serverConversationId: "archived",
      archived: false,
      title: "Archived",
    }),
  )
  expect(f.opens).toHaveLength(4)
  expect(f.state().archivedIds).toEqual([])
  f.frame(0, [])
  f.frame(1, ["archived"])
  expect(f.state().archivedIds).toEqual([])
  f.frame(2, ["archived"])
  f.frame(3, [])
  expect(f.state().rows?.[0]?.conversationId).toBe("archived")
  f.stop()
  expect(f.opens.map((x) => x.stop.mock.calls.length)).toEqual([1, 1, 1, 1])
})

it("PL4: stale failures after cleanup produce neither publication nor diagnostics", () => {
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
  const f = fixture()
  f.frame(0, ["current"])
  f.frame(1, [])
  f.stop()
  f.opens[0]!.follower.failed("unavailable")
  f.opens[1]!.follower.failed("state-unreadable")
  expect(f.state().failure).toBeNull()
  expect(warn).not.toHaveBeenCalled()
  warn.mockRestore()
})

it("PL6: a refreshed pair ignores old failures before diagnosing them", async () => {
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
  try {
    const f = fixture({ archive: async () => true })
    f.frame(0, ["active"])
    f.frame(1, [])
    await f.store.dispatch(
      archiveConversation({
        serverConversationId: "active",
        archived: true,
        title: "Active",
      }),
    )
    f.frame(2, [])
    f.frame(3, ["active"])
    f.opens[0]!.follower.failed("unavailable")
    f.opens[1]!.follower.failed("state-unreadable")
    expect(f.state().failure).toBeNull()
    expect(warn).not.toHaveBeenCalled()
    f.stop()
  } finally {
    warn.mockRestore()
  }
})
