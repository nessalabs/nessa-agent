import { describe, expect, it } from "vitest"
import { createReplyStyleApi, replyStyles } from "./reply-style-api.js"

const plain = { mode: "plain" } as const
const answer = {
  channelId: "ch_aaaaaaaaaa",
  nessa: { thread: { mode: "custom", text: "Be brief." } },
  channel: { coordinator: { mode: "off" } },
  effective: { coordinator: { mode: "off" }, thread: { mode: "custom", text: "Be brief." } },
}

function recording(reply: unknown) {
  const asked: unknown[] = []
  const api = createReplyStyleApi({
    request: async (method: string, params: unknown) => {
      asked.push([method, params])
      return reply
    },
  } as never)
  return { api, asked }
}

describe("replyStyle", () => {
  it("reads the Nessa level, or one channel's", async () => {
    const { api, asked } = recording(answer)
    expect(await api.read("ch_aaaaaaaaaa")).toEqual(answer)
    await api.read()
    expect(asked).toEqual([
      ["replyStyle.read", { channelId: "ch_aaaaaaaaaa" }],
      ["replyStyle.read", {}],
    ])
  })

  it("saves a choice, and sends default to follow the level above", async () => {
    const { api, asked } = recording({
      nessa: {},
      effective: { coordinator: plain, thread: plain },
    })
    await api.set("thread", { mode: "custom", text: "Be brief." })
    await api.set("coordinator", undefined, "ch_aaaaaaaaaa")
    expect(asked).toEqual([
      ["replyStyle.set", { role: "thread", mode: "custom", text: "Be brief." }],
      ["replyStyle.set", { channelId: "ch_aaaaaaaaaa", role: "coordinator", mode: "default" }],
    ])
  })

  it("refuses an answer off its schema", () => {
    for (const bad of [
      { nessa: {} },
      { nessa: {}, effective: { coordinator: plain } },
      { nessa: { owner: plain }, effective: { coordinator: plain, thread: plain } },
      { nessa: {}, effective: { coordinator: { mode: "plain", text: "x" }, thread: plain } },
      { nessa: {}, effective: { coordinator: { mode: "custom" }, thread: plain } },
      { nessa: {}, effective: { coordinator: { mode: "custom", text: "" }, thread: plain } },
      {
        nessa: {},
        effective: { coordinator: { mode: "custom", text: "é".repeat(2049) }, thread: plain },
      },
      { channelId: "ch_aaaaaaaaaa", nessa: {}, effective: { coordinator: plain, thread: plain } },
      { nessa: {}, channel: {}, effective: { coordinator: plain, thread: plain } },
    ])
      expect(() => replyStyles(bad)).toThrow("Invalid reply styles")
  })
})
