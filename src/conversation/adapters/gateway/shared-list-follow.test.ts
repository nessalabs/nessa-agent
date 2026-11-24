import { describe, expect, it, vi } from "vitest"
import type { ConversationListFollower } from "../../application/ports"
import type { ConversationListing } from "../../application/view"
import { sharedListFollows } from "./shared-list-follow"

const listing = (title: string): ConversationListing => ({
  conversations: [
    {
      conversationId: "c1",
      title,
      preview: null,
      updatedAtMs: 1,
      running: false,
      archived: false,
    },
  ],
  complete: true,
})

const follower = () => {
  const lists: string[] = []
  const failures: string[] = []
  const it: ConversationListFollower = {
    list: (list) => lists.push(list.conversations[0]?.title ?? ""),
    failed: (reason) => failures.push(String(reason)),
  }
  return { it, lists, failures }
}

describe("sharedListFollows", () => {
  it("opens one gateway follow per list, tells every follower, and closes after the last", async () => {
    let told: ConversationListFollower | undefined
    const stop = vi.fn()
    const open = vi.fn((_archived: boolean, each: ConversationListFollower) => {
      told = each
      return stop
    })
    const follow = sharedListFollows(open)
    const messages = follower()
    const asks = follower()
    const stopMessages = follow(false, messages.it)
    told?.list(listing("first"))
    const stopAsks = follow(false, asks.it)
    await Promise.resolve()
    expect(open).toHaveBeenCalledTimes(1)
    expect(asks.lists).toEqual(["first"])
    told?.list(listing("second"))
    told?.failed("not-connected" as never)
    expect(messages.lists).toEqual(["first", "second"])
    expect(asks.lists).toEqual(["first", "second"])
    expect(asks.failures).toEqual(["not-connected"])
    stopMessages()
    stopMessages()
    expect(stop).not.toHaveBeenCalled()
    stopAsks()
    expect(stop).toHaveBeenCalledTimes(1)
    follow(false, follower().it)
    expect(open).toHaveBeenCalledTimes(2)
  })

  it("keeps archived and current lists apart", () => {
    const open = vi.fn((_archived: boolean, _each: ConversationListFollower) => () => {})
    const follow = sharedListFollows(open)
    follow(false, follower().it)
    follow(true, follower().it)
    expect(open.mock.calls.map(([archived]) => archived)).toEqual([false, true])
  })

  it("does not tell a follower that stopped before the last list reached it", async () => {
    let told: ConversationListFollower | undefined
    const follow = sharedListFollows((_a, each) => {
      told = each
      return () => {}
    })
    follow(false, follower().it)
    told?.list(listing("first"))
    const late = follower()
    follow(false, late.it)()
    await Promise.resolve()
    expect(late.lists).toEqual([])
  })
})
