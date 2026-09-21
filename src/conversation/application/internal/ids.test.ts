/**
 * That opening a conversation cannot hand out an id already on screen.
 *
 * The tab strip keys every tab by its conversation id, and React duplicates
 * the children under a repeated key rather than refusing it. That is what a
 * panel full of "Queued 1" and "Queued 2" chips was: duplicated children, each
 * frozen at the count of the render that made it, none of them removed when
 * the queue drained.
 *
 * The counter is a separate field from the conversations it names, so these
 * drive it out of step on purpose — which a restore, or anything replacing the
 * list wholesale, can do without meaning to.
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

  it("moves the counter past what it found, so the next call does not rescan", () => {
    const { tabs } = takeConversationId(tabsWith(["c0", "c1", "c2"], 0))
    expect(tabs.nextConversationId).toBe(4)
  })
})
