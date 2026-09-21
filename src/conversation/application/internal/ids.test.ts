/**
 * That opening a conversation cannot hand out an id already on screen.
 *
 * Ids key the tab strip, and a repeated key is a defect the renderer will not
 * refuse — see src/panel/ui/composer-keys.test.tsx for what that costs. No
 * path builds a counter out of step with its conversations today, so these
 * drive it there on purpose: the guard is for the state the type permits, not
 * for one that has been observed.
 */
import { describe, expect, it } from "vitest"

import { conversation } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { takeConversationId } from "./ids"

function tabsWith(ids: string[], nextConversationId: number): LocalTabs {
  return {
    conversations: ids.map((id) => conversation(id)),
    activeId: ids[0] ?? "c0",
    nextConversationId,
    nextTurnId: 1,
  }
}

describe("taking a conversation id", () => {
  it("refuses an id a conversation already answers to", () => {
    // The counter has fallen a step behind: `c0` is open, and it still says 0.
    const { id, tabs } = takeConversationId(tabsWith(["c0"], 0))
    expect(id).not.toBe("c0")
    expect(tabs.conversations.map((item) => item.id)).not.toContain(id)
  })

  it("keeps every id distinct however far behind the counter is", () => {
    let tabs = tabsWith(["c0", "c1", "c2"], 0)
    const handed: string[] = []
    for (let n = 0; n < 4; n += 1) {
      const taken = takeConversationId(tabs)
      handed.push(taken.id)
      tabs = {
        ...taken.tabs,
        conversations: [...taken.tabs.conversations, conversation(taken.id)],
      }
    }
    expect(new Set(handed).size).toBe(handed.length)
    const all = tabs.conversations.map((item) => item.id)
    expect(new Set(all).size).toBe(all.length)
  })

  it("takes the counter's own answer when nothing has claimed it", () => {
    // The ordinary path is unchanged: no scan, no gap in the numbering.
    const { id, tabs } = takeConversationId(tabsWith(["c0"], 1))
    expect(id).toBe("c1")
    expect(tabs.nextConversationId).toBe(2)
  })

  it("does not rescan from the same place twice", () => {
    // The invariant, not the number: a second call must not walk the ids the
    // first one already walked, whatever the counter happens to hold.
    const first = takeConversationId(tabsWith(["c0", "c1", "c2"], 0))
    const second = takeConversationId({
      ...first.tabs,
      conversations: [...first.tabs.conversations, conversation(first.id)],
    })
    expect(second.id).not.toBe(first.id)
    expect(second.tabs.nextConversationId).toBeGreaterThan(first.tabs.nextConversationId)
  })
})
