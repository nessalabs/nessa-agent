import { expect, it } from "vitest"

import { conversation } from "../model"
import { emptyLocalTabs } from "./local-tabs"
import { conversationTabSnapshot, restoreConversationTabs } from "./saved-tabs"
import { openConversation, closeConversation } from "./usecases"

const kept = "0b8f1c2e-1111-4a4a-8b8b-000000000001"
const deleted = "0b8f1c2e-1111-4a4a-8b8b-000000000002"

it("does not bring back a tab onto a conversation that was deleted", () => {
  const tabs = {
    ...emptyLocalTabs(),
    conversations: [
      { ...conversation("c0"), serverConversationId: kept },
      {
        ...conversation("c1"),
        serverConversationId: deleted,
        readError: "deleted" as const,
      },
    ],
    activeId: "c1",
  }
  expect(conversationTabSnapshot(tabs)).toEqual({ tabs: [{ conversationId: kept }] })
})

it("LT7: restoration uses fresh local lifetimes while preserving remote selection and titles", () => {
  const original = { ...emptyLocalTabs(), nextTurnId: 17 }
  const restored = restoreConversationTabs(original, {
    tabs: [{ conversationId: kept, title: "Kept" }, { conversationId: deleted }],
    activeConversationId: deleted,
  })
  expect(restored.conversations.map((tab) => tab.id)).toEqual(["c1", "c2"])
  expect(restored.activeId).toBe("c2")
  expect(restored.nextConversationId).toBe(3)
  expect(restored.nextTurnId).toBe(17)
  expect(restored.conversations[0]).toMatchObject({
    title: "Kept",
    titleEdited: true,
    serverConversationId: kept,
  })
  const again = restoreConversationTabs(restored, { tabs: [{ conversationId: kept }] })
  expect(again.activeId).toBe("c3")
  const opened = openConversation(again)
  expect(opened.activeId).toBe("c4")
  const closed = closeConversation(opened, "c4")
  const replaced = restoreConversationTabs(closed, { tabs: [] })
  expect(replaced.activeId).toBe("c5")
  expect(replaced.nextConversationId).toBe(6)
  expect(replaced.nextTurnId).toBe(17)
  expect(replaced.conversations[0].serverConversationId).toBeUndefined()
})

it("LT7: allocation also respects live names when a supplied counter lags", () => {
  const original = {
    ...emptyLocalTabs(),
    conversations: [conversation("c1"), conversation("c2")],
    nextConversationId: 1,
  }
  const restored = restoreConversationTabs(original, {
    tabs: [{ conversationId: kept }, { conversationId: deleted }],
  })
  expect(restored.conversations.map((tab) => tab.id)).toEqual(["c3", "c4"])
  expect(restored.nextConversationId).toBe(5)
})
