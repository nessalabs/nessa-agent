import { describe, expect, it, vi } from "vitest"

const catalogue = vi.hoisted(() => ({ empty: false }))
vi.mock("../../model/composer-options", async (load) => {
  const real = await load<typeof import("../../model/composer-options")>()
  return {
    ...real,
    get composerModels() {
      return catalogue.empty ? [] : real.composerModels
    },
  }
})

const { consistentIndex, contradicts, defaultModel, nowLine } =
  await import("./workspace-index")
const { defaultComposerModel, composerModels } =
  await import("../../model/composer-options")

describe("the model a new session starts on", () => {
  it("is the composer's own default, not a second copy of it", () => {
    catalogue.empty = false
    const composer = defaultComposerModel(composerModels)
    expect(defaultModel()).toEqual({
      provider: composer?.provider,
      modelId: composer?.modelId,
    })
  })

  it("is nothing when the catalogue has no model: no session can start", () => {
    catalogue.empty = true
    expect(defaultModel()).toBeUndefined()
    catalogue.empty = false
  })
})

describe("the index the workspace takes", () => {
  const section = (id: string) => ({ id, name: id })
  const channel = (id: string, sectionId: string) => ({
    id,
    name: id,
    sectionId,
    private: false,
    topic: "",
  })
  const session = (id: string, channelId: string) => ({
    id,
    channelId,
    title: id,
    agent: "claude" as const,
    model: { provider: "anthropic", modelId: "claude-opus-5" },
    status: "idle" as const,
    startedAt: 1,
    updatedAt: 1,
    preview: "",
    pinned: false,
    unread: false,
    revision: 1,
  })

  it("takes an index that contradicts nothing as it is", () => {
    const index = {
      sections: [section("s")],
      channels: [channel("c", "s")],
      sessions: [session("a", "c")],
    }
    const taken = consistentIndex(index)
    expect(taken.index).toEqual(index)
    expect(contradicts(taken.contradictions)).toBe(false)
  })

  it("leaves out a channel under no section it lists, and the sessions in it", () => {
    const taken = consistentIndex({
      sections: [section("s")],
      channels: [channel("c", "s"), channel("orphan", "gone")],
      sessions: [session("a", "c"), session("stranded", "orphan")],
    })
    expect(taken.index.channels.map((c) => c.id)).toEqual(["c"])
    expect(taken.index.sessions.map((c) => c.id)).toEqual(["a"])
    expect(taken.contradictions).toEqual({
      sections: [],
      channels: ["orphan"],
      sessions: ["stranded"],
    })
  })

  it("keeps the first of an id listed twice, section, channel or session", () => {
    const taken = consistentIndex({
      sections: [section("s"), { id: "s", name: "again" }],
      channels: [channel("c", "s"), { ...channel("c", "s"), name: "again" }],
      sessions: [session("a", "c"), { ...session("a", "c"), title: "again" }],
    })
    expect(taken.index.sections).toEqual([section("s")])
    expect(taken.index.channels).toEqual([channel("c", "s")])
    expect(taken.index.sessions.map((s) => s.title)).toEqual(["a"])
    expect(taken.contradictions).toEqual({
      sections: ["s"],
      channels: ["c"],
      sessions: ["a"],
    })
  })
})

describe("the source's line of what is going on", () => {
  const summaryWith = (now: string | undefined) => ({
    id: "s",
    channelId: "c",
    title: "S",
    model: { provider: "anthropic", modelId: "claude-opus-5" },
    status: "running" as const,
    startedAt: 1,
    updatedAt: 2,
    preview: "",
    now,
    pinned: false,
    unread: false,
    revision: 1,
  })

  it("is the line as the source sent it, when it says something", () => {
    expect(nowLine(summaryWith("  Running the tests\n"))).toBe("  Running the tests\n")
  })

  it("is nothing when absent, empty, or only whitespace", () => {
    for (const blank of [undefined, "", " ", "\n", "\t \u00a0"])
      expect(nowLine(summaryWith(blank))).toBeNull()
  })
})
