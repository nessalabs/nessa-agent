import { describe, expect, it, vi } from "vitest"
import {
  channelRules as publishedRules,
  NessaRpcError,
  type Channel as WireChannel,
} from "@nessa/client"
import { ChannelCreationError } from "../../application/ports"
import { channelRules, emptyDraft } from "../../model/channel-draft"
import { channelGateway } from "./channel-gateway"
import { gatewayChannel } from "./gateway-views"

const model = { provider: "openai", modelId: "gpt-5.6-terra" }

const made: WireChannel = {
  channelId: "ch_abcdefgh27",
  name: "design-review",
  folder: "/Users/me/.nessa/channels/ch_abcdefgh27",
  folderMade: true,
  agent: "codex",
  model: "gpt-5.6-terra",
  approvalMode: "auto",
  coordinatorConversationId: null,
  coordinatorPlacement: "local",
  threadPlacement: "local",
  createdAtMs: 1,
}

describe("making a channel on the gateway", () => {
  it("asks with the draft in the protocol's words and answers the index's channel", async () => {
    const create = vi.fn().mockResolvedValue(made)
    const source = channelGateway({ connected: async () => ({ channels: { create } }) })
    const channel = await source.create(
      {
        ...emptyDraft(model),
        name: "  design-review ",
        access: "auto",
        notes: "Check every export.",
        resources: ["/Users/me/brand-kit"],
      },
      "request-1",
    )
    expect(create).toHaveBeenCalledWith({
      requestId: "request-1",
      name: "design-review",
      agent: "codex",
      model: "gpt-5.6-terra",
      coordinatorPlacement: "local",
      threadPlacement: "local",
      approvalMode: "auto",
      notes: "Check every export.",
      resources: ["/Users/me/brand-kit"],
    })
    expect(channel).toMatchObject({
      id: "ch_abcdefgh27",
      name: "design-review",
      folder: made.folder,
      defaults: { model, access: "auto" },
    })
  })

  it("names a chosen folder and leaves out what was left empty", async () => {
    const create = vi
      .fn()
      .mockResolvedValue({ ...made, folderMade: false, folder: "/repo" })
    const source = channelGateway({ connected: async () => ({ channels: { create } }) })
    await source.create({ ...emptyDraft(model), name: "x", folder: "/repo" }, "r")
    expect(create.mock.calls[0]?.[0]).toEqual({
      requestId: "r",
      name: "x",
      folder: "/repo",
      agent: "codex",
      model: "gpt-5.6-terra",
      coordinatorPlacement: "local",
      threadPlacement: "local",
      approvalMode: "ask",
    })
  })

  it("says each refusal by kind, and anything else as unavailable", async () => {
    const refusing = (error: unknown) =>
      channelGateway({
        connected: async () => ({
          channels: { create: vi.fn().mockRejectedValue(error) },
        }),
      }).create({ ...emptyDraft(model), name: "x" }, "r")
    await expect(
      refusing(new NessaRpcError("channel_folder_refused", "refused")),
    ).rejects.toEqual(new ChannelCreationError("folder-refused"))
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    await expect(refusing(new Error("lost"))).rejects.toEqual(
      new ChannelCreationError("unavailable"),
    )
    expect(error).toHaveBeenCalled()
    error.mockRestore()
  })
})

describe("the flow's bounds", () => {
  it("are the ones the client publishes from the protocol", () => {
    const { maxChannels: _, ...asked } = publishedRules
    expect(channelRules).toEqual(asked)
  })

  it("changes a made channel's run location and answers the index's channel", async () => {
    const setPlacement = vi.fn().mockResolvedValue({
      ...made,
      coordinatorPlacement: "boat",
      coordinatorConversationId: "fresh-main",
    })
    const source = channelGateway({
      connected: async () => ({ channels: { create: vi.fn(), setPlacement } }),
    })
    const channel = await source.setPlacement?.(
      made.channelId,
      { coordinatorPlacement: "boat", threadPlacement: "local" },
      "req-place",
    )
    expect(setPlacement).toHaveBeenCalledWith({
      requestId: "req-place",
      channelId: made.channelId,
      coordinatorPlacement: "boat",
      threadPlacement: "local",
    })
    expect(channel?.coordinatorPlacement).toBe("boat")
    expect(channel?.coordinatorConversationId).toBe("fresh-main")
  })

  it("refuses to place the default channel without asking", async () => {
    const setPlacement = vi.fn()
    const source = channelGateway({
      connected: async () => ({ channels: { create: vi.fn(), setPlacement } }),
    })
    await expect(
      source.setPlacement?.(
        gatewayChannel.id,
        { coordinatorPlacement: "boat", threadPlacement: "boat" },
        "r",
      ),
    ).rejects.toMatchObject({ reason: "forbidden" })
    expect(setPlacement).not.toHaveBeenCalled()
  })
})
