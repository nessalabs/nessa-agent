import { expect, it } from "vitest"

import { conversation } from "../model"
import { emptyLocalTabs } from "./local-tabs"
import { conversationTabSnapshot } from "./saved-tabs"

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
