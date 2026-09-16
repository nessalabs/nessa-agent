import { describe, expect, it } from "vitest"
import { createDependencies } from "./dependencies"
import { scenarioEffects } from "../conversation/adapters/scenario/effects"
import { makeStore } from "../store"
import { sendDraft } from "../conversation/adapters/store/slice"

describe("application dependency scope", () => {
  it("routes real conversation operations through each injected adapter without sharing state", async () => {
    const first = makeStore(createDependencies({ conversation: scenarioEffects("echo") }))
    const second = makeStore(
      createDependencies({ conversation: scenarioEffects("echo") }),
    )
    await Promise.all([
      first.dispatch(sendDraft({ content: [{ type: "text", text: "first" }] })),
      second.dispatch(sendDraft({ content: [{ type: "text", text: "second" }] })),
    ])
    expect(first.getState().conversation.conversations[0]!.turns[1]).toMatchObject({
      text: "first",
    })
    expect(second.getState().conversation.conversations[0]!.turns[1]).toMatchObject({
      text: "second",
    })
    expect(first.getState().conversation.conversations[0]!.serverConversationId).not.toBe(
      second.getState().conversation.conversations[0]!.serverConversationId,
    )
    const disconnected = makeStore()
    await disconnected.dispatch(sendDraft({ content: [{ type: "text", text: "hello" }] }))
    expect(disconnected.getState().conversation.conversations[0]!.turns[0]).toMatchObject(
      { receipt: "failed" },
    )
    expect(disconnected.getState().conversation.conversations[0]!.turns).toHaveLength(1)
  })
})
