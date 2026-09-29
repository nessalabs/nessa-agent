import { describe, expect, it } from "vitest"
import catalog from "../../../crates/nessa-sdk/data/models.json"
import {
  agentForProvider,
  composerModels,
  contextLabel,
  defaultComposerModel,
  groupByProvider,
  providerLabel,
  thinkingLevelsFor,
  offeredLevelIndex,
  levelRank,
  type ComposerModel,
  shortModelName,
} from "./composer-options"

function model(
  provider: string,
  modelId: string,
  effortLevels: readonly string[] | null = ["low", "medium", "high", "max"],
): ComposerModel {
  return {
    provider,
    modelId,
    displayName: modelId,
    reasoning: effortLevels === null ? null : { effortLevels },
    fastMode: false,
    maxContextWindowTokens: 200_000,
  }
}
const values = (levels: readonly { value: string }[]) =>
  levels.map((level) => level.value)

describe("groupByProvider", () => {
  it("keeps catalog order within and between providers", () => {
    const groups = groupByProvider([
      model("openai", "a"),
      model("anthropic", "b"),
      model("openai", "c"),
    ])
    expect(groups.map((group) => [group.id, group.models.map((m) => m.modelId)])).toEqual(
      [
        ["openai", ["a", "c"]],
        ["anthropic", ["b"]],
      ],
    )
  })
})

describe("providerLabel", () => {
  it("names known providers and shows an unknown one by its id", () => {
    expect(providerLabel("openai")).toBe("OpenAI")
    expect(providerLabel("toString")).toBe("toString")
  })
})

describe("agentForProvider", () => {
  it("maps each catalog provider to the agent that runs it, and none otherwise", () => {
    expect(agentForProvider("anthropic")).toBe("claude")
    expect(agentForProvider("openai")).toBe("codex")
    expect(agentForProvider("opencode")).toBe("opencode")
    expect(agentForProvider("toString")).toBeUndefined()
  })
})

describe("defaultComposerModel", () => {
  it("opens on Claude Opus 5 from the SDK catalog", () => {
    expect(defaultComposerModel(composerModels)?.modelId).toBe("claude-opus-5")
  })

  it("falls back to the first model when Opus 5 is absent, and to none when empty", () => {
    expect(defaultComposerModel([model("openai", "a")])?.modelId).toBe("a")
    expect(defaultComposerModel([])).toBeUndefined()
  })
})

describe("contextLabel", () => {
  it.each([
    [1_050_000, "1M context"],
    [200_000, "200K context"],
    [128_000, "128K context"],
  ])("labels %i tokens as %s", (tokens, label) => {
    expect(contextLabel(tokens)).toBe(label)
  })
})

describe("thinkingLevelsFor", () => {
  it("offers no levels to a model that does not reason, or has none recorded", () => {
    expect(thinkingLevelsFor(model("x", "y", null))).toEqual([])
    expect(thinkingLevelsFor(model("x", "y", []))).toEqual([])
    expect(thinkingLevelsFor(undefined)).toEqual([])
  })

  it("offers exactly the levels the model publishes, in its provider's order", () => {
    const levels = thinkingLevelsFor(model("x", "y", ["none", "low", "xhigh", "max"]))
    expect(values(levels)).toEqual(["none", "low", "xhigh", "max"])
    expect(levels.map((level) => level.label)).toEqual([
      "None",
      "Low",
      "Extra high",
      "Max",
    ])
    expect(levels.every((level) => level.description.length > 0)).toBe(true)
    expect(levels.some((level) => level.utmost)).toBe(false)
  })

  it("marks only a level published past Max as Ultra, the utmost", () => {
    const levels = thinkingLevelsFor(model("x", "y", ["low", "max", "ultra"]))
    expect(levels.map((level) => level.utmost ?? false)).toEqual([false, false, true])
    expect(levels[2]).toMatchObject({ label: "Ultra", utmost: true })
    // A name the control does not word is shown as its provider writes it.
    expect(thinkingLevelsFor(model("x", "y", ["turbo"]))[0]).toMatchObject({
      value: "turbo",
      label: "Turbo",
    })
  })

  it("offers each catalogue model the levels and Fast mode its entry records", () => {
    for (const entry of catalog.models) {
      const composer = composerModels.find(
        (known) => known.provider === entry.provider && known.modelId === entry.modelId,
      )
      expect(values(thinkingLevelsFor(composer)), entry.modelId).toEqual(
        entry.reasoning?.effortLevels ?? [],
      )
      expect(composer?.fastMode, entry.modelId).toBe(entry.fastMode)
    }
  })
})

describe("levelRank", () => {
  it("stands each worded level where it is among them all, whichever model offers it", () => {
    expect(levelRank("none")).toBe(0)
    expect(levelRank("low")).toBe(1)
    expect(levelRank("xhigh")).toBe(4)
    expect(levelRank("max")).toBe(5)
    expect(levelRank("turbo")).toBe(-1)
    expect(levelRank(undefined)).toBe(-1)
  })
})

describe("offeredLevelIndex", () => {
  const upToMax = thinkingLevelsFor(model("x", "y"))
  const wide = thinkingLevelsFor(
    model("x", "y", ["none", "low", "medium", "high", "xhigh", "max", "ultra"]),
  )

  it("is the level itself where the model offers it", () => {
    expect(offeredLevelIndex(upToMax, "high")).toBe(2)
    expect(offeredLevelIndex(wide, "ultra")).toBe(6)
  })

  it("shows a level the model lacks as the highest it offers below it", () => {
    expect(offeredLevelIndex(upToMax, "xhigh")).toBe(2)
    // Below all it offers: its least.
    expect(offeredLevelIndex(upToMax, "none")).toBe(0)
    // A level past Max stands below nothing, so a carried High is never shown as it.
    expect(
      offeredLevelIndex(
        thinkingLevelsFor(model("x", "y", ["low", "max", "ultra"])),
        "high",
      ),
    ).toBe(0)
  })

  it("shows a value it does not word as the first level", () => {
    expect(offeredLevelIndex(upToMax, "turbo")).toBe(0)
    expect(offeredLevelIndex(upToMax, "ultra")).toBe(0)
    expect(offeredLevelIndex([], "high")).toBe(0)
  })
})

describe("a model's short name", () => {
  const model = (provider: string, displayName: string) => ({
    provider,
    modelId: displayName,
    displayName,
    reasoning: null,
    fastMode: false,
    maxContextWindowTokens: 1,
  })
  const catalogue = [
    model("anthropic", "Claude Opus 5"),
    model("anthropic", "Claude Sonnet 5"),
    model("openai", "GPT-6 Astra"),
    model("openai", "GPT-5.6 Sol"),
    model("solo", "Solo One"),
  ]

  it("drops the word a provider's models all start with, so they still read apart", () => {
    expect(shortModelName(catalogue[0], catalogue)).toBe("Opus 5")
    expect(shortModelName(catalogue[1], catalogue)).toBe("Sonnet 5")
  })

  it("keeps a name whole where nothing is shared, or it is the only one", () => {
    expect(shortModelName(catalogue[2], catalogue)).toBe("GPT-6 Astra")
    expect(shortModelName(catalogue[4], catalogue)).toBe("Solo One")
  })
})
