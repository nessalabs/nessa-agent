import { describe, expect, it } from "vitest"
import { createDependencies } from "./dependencies"
import { makeStore } from "../store"
import { sendDraft } from "../conversation/adapters/store/slice"

describe("application dependency scope", () => {
  it("routes commands through each injected adapter without sharing state", async () => {
    const first = makeStore(
      createDependencies({ conversation: { echo: async () => ({ text: "first" }) } }),
    )
    const second = makeStore(
      createDependencies({ conversation: { echo: async () => ({ text: "second" }) } }),
    )
    const [a, b] = await Promise.all([
      first.dispatch(sendDraft({ text: "hello" })),
      second.dispatch(sendDraft({ text: "hello" })),
    ])
    expect(a.payload).toEqual({ text: "first" })
    expect(b.payload).toEqual({ text: "second" })
    const disconnected = makeStore()
    const failure = await disconnected.dispatch(sendDraft({ text: "hello" }))
    expect(sendDraft.rejected.match(failure)).toBe(true)
  })
})
