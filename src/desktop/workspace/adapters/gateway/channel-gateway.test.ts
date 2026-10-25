import { describe, expect, it, vi } from "vitest"
import { NessaRpcError, type Channel as WireChannel } from "@nessa/client"
import { ChannelCreationError } from "../../application/ports"
import { emptyDraft } from "../../model/channel-draft"
import { channelGateway } from "./channel-gateway"

const model = { provider: "openai", modelId: "gpt-5.6-terra" }

const made: WireChannel = {
  channelId: "ch_abcdefgh27",
  name: "design-review",
  folder: "/Users/me/.nessa/channels/ch_abcdefgh27",
  folderMade: true,
  agent: "codex",
  model: "gpt-5.6-terra",
  approvalMode: "auto",
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
