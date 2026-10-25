import { describe, expect, it } from "vitest"
import {
  canCreate,
  channelName,
  channelRules,
  channelsIn,
  emptyDraft,
  failureStep,
  homeShort,
  pathName,
  withResources,
} from "./channel-draft"
import type { Channel } from "./workspace-index"

const model = { provider: "openai", modelId: "gpt-5.6-terra" }

describe("a channel being made", () => {
  it("keeps a trimmed name of at most 80 characters, with no control characters", () => {
    expect(channelName("  design ")).toBe("design")
    expect(channelName("   ")).toBeNull()
    expect(channelName("a\nb")).toBeNull()
    expect(channelName("é".repeat(channelRules.maxNameChars))).not.toBeNull()
    expect(channelName("é".repeat(channelRules.maxNameChars + 1))).toBeNull()
  })

  it("can be made once it has a name and its notes fit", () => {
    const draft = emptyDraft(model)
    expect(canCreate(draft)).toBe(false)
    expect(canCreate({ ...draft, name: "design" })).toBe(true)
    expect(
      canCreate({
        ...draft,
        name: "design",
        notes: "a".repeat(channelRules.maxNotesBytes + 1),
      }),
    ).toBe(false)
  })

  it("adds each resource once and leaves out what does not fit", () => {
    expect(withResources(["/a"], ["/a", "/b"])).toEqual({
      resources: ["/a", "/b"],
      leftOut: 0,
    })
    const many = Array.from({ length: 40 }, (_, n) => `/r${n}`)
    const added = withResources([], many)
    expect(added.resources).toHaveLength(channelRules.maxResources)
    expect(added.leftOut).toBe(8)
    const long = Array.from({ length: 5 }, (_, n) => `/${n}${"a".repeat(4000)}`)
    expect(withResources([], long).leftOut).toBe(1)
  })

  it("names the other channels that work in a folder", () => {
    const channel = (id: string, folder?: string): Channel => ({
      id,
      name: id,
      sectionId: "gateway",
      private: false,
      topic: "",
      folder,
    })
    const channels = [channel("a", "/repo"), channel("b", "/other"), channel("c")]
    expect(channelsIn(channels, "/repo").map((found) => found.id)).toEqual(["a"])
    expect(channelsIn(channels, null)).toEqual([])
  })

  it("writes paths the way a row shows them", () => {
    expect(pathName("/Users/me/Design/brand-kit/")).toBe("brand-kit")
    expect(pathName("C:\\Users\\me\\tokens.json")).toBe("tokens.json")
    expect(homeShort("/Users/me/Design", "/Users/me")).toBe("~/Design")
    expect(homeShort("/Users/meg/Design", "/Users/me")).toBe("/Users/meg/Design")
  })

  it("sends a folder failure back to the folder question", () => {
    expect(failureStep("folder-refused")).toBe("folder")
    expect(failureStep("resource-not-found")).toBe("resources")
    expect(failureStep("unavailable")).toBeNull()
  })
})
