import { describe, expect, it, vi } from "vitest"
import { NessaRpcError } from "../application/rpc-error.js"
import type { Channel } from "../generated/product.js"
import { channelRefusal, createChannelApi } from "./channel-api.js"

const made: Channel = {
  channelId: "ch_abcdefgh27",
  name: "design-review",
  folder: "/Users/me/.nessa/channels/ch_abcdefgh27",
  folderMade: true,
  agent: "codex",
  model: "gpt-5.6-terra",
  approvalMode: "auto",
  coordinatorPlacement: "local",
  threadPlacement: "local",
  coordinatorConversationId: null,
  createdAtMs: 1000,
}

const conversations: Channel = {
  channelId: "default",
  name: "Conversations",
  folder: "/Users/me/.nessa/workspaces/default",
  folderMade: false,
  agent: null,
  model: null,
  approvalMode: "ask",
  coordinatorPlacement: "local",
  threadPlacement: "local",
  coordinatorConversationId: null,
  createdAtMs: 0,
}

describe("the person's channels", () => {
  it("makes a channel and lists them, the default first", async () => {
    const request = vi
      .fn()
      .mockResolvedValueOnce({ channel: made })
      .mockResolvedValueOnce({ channels: [conversations, made], complete: true })
    const api = createChannelApi({ request })
    const params = {
      requestId: "r1",
      name: "design-review",
      agent: "codex",
      model: "gpt-5.6-terra",
      approvalMode: "auto" as const,
      resources: ["/Users/me/brand-kit"],
    }
    expect(await api.create(params)).toEqual(made)
    expect(request).toHaveBeenCalledWith("channel.create", params)
    expect(await api.list()).toEqual({ channels: [conversations, made], complete: true })
    expect(request).toHaveBeenLastCalledWith("channel.list", {})
  })

  it("refuses an answer that is not the protocol's", async () => {
    for (const answer of [
      null,
      { channel: { ...made, channelId: "ch_bad" } },
      { channel: { ...made, approvalMode: "never" } },
      { channel: { ...made, notes: "listed" } },
      { channel: { ...made, createdAtMs: -1 } },
    ]) {
      const api = createChannelApi({ request: vi.fn().mockResolvedValue(answer) })
      await expect(
        api.create({ requestId: "r", name: "x", agent: "codex", model: "m" }),
      ).rejects.toThrow("Invalid channel create response")
    }
    const api = createChannelApi({
      request: vi.fn().mockResolvedValue({ channels: [made] }),
    })
    await expect(api.list()).rejects.toThrow("Invalid channel list response")
  })

  it("reads every page of a list the gateway cut, and refuses a cursor that goes nowhere", async () => {
    const later = { ...made, channelId: "ch_bbbbbbbbbb", name: "later" }
    const request = vi
      .fn()
      .mockResolvedValueOnce({
        channels: [conversations, made],
        complete: false,
        cursor: made.channelId,
      })
      .mockResolvedValueOnce({ channels: [later], complete: true })
    const api = createChannelApi({ request })
    expect(await api.list()).toEqual({
      channels: [conversations, made, later],
      complete: true,
    })
    expect(request).toHaveBeenNthCalledWith(1, "channel.list", {})
    expect(request).toHaveBeenNthCalledWith(2, "channel.list", { after: made.channelId })
    for (const pages of [
      // The same cursor again would never end.
      [
        { channels: [made], complete: false, cursor: made.channelId },
        { channels: [], complete: false, cursor: made.channelId },
      ],
      // A cut page with no cursor, a whole one with one, and a default cursor.
      [{ channels: [made], complete: false }],
      [{ channels: [made], complete: true, cursor: made.channelId }],
      [{ channels: [conversations], complete: false, cursor: "default" }],
    ]) {
      const answers = vi.fn()
      for (const page of pages) answers.mockResolvedValueOnce(page)
      await expect(createChannelApi({ request: answers }).list()).rejects.toThrow(
        "Invalid channel list response",
      )
    }
  })

  it("reads a refusal's code when this build knows it", () => {
    expect(channelRefusal(new NessaRpcError("channel_folder_refused", "refused"))).toBe(
      "channel_folder_refused",
    )
    expect(channelRefusal(new NessaRpcError("something_else", "x"))).toBeUndefined()
    expect(channelRefusal(new Error("x"))).toBeUndefined()
  })
})
