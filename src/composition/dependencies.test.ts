import { describe, expect, it, vi } from "vitest"
import { createDependencies } from "./dependencies"
import { scenarioEffects } from "../conversation/adapters/scenario/effects"
import { makeStore } from "../store"
import { sendDraft } from "../conversation/adapters/store/slice"
import { sha256Digest } from "../panel/adapters/sha256"

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

  it("hands out the digest an upload is identified by, and takes a substitute for it", async () => {
    // Hashing reads Web Crypto, which is outside this process like a socket or
    // a clock, so it arrives from here rather than being imported where it is
    // used — and a test can hash without `crypto.subtle` anywhere near it.
    const bytes = new Blob(["abc"])
    expect(await createDependencies().digest(bytes)).toBe(await sha256Digest(bytes))
    const substitute = vi.fn(async () => `sha256:${"cd".repeat(32)}`)
    const scoped = createDependencies({ digest: substitute })
    expect(await scoped.digest(bytes)).toBe(`sha256:${"cd".repeat(32)}`)
    expect(substitute).toHaveBeenCalledExactlyOnceWith(bytes)
  })
})
