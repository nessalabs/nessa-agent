import { describe, expect, it } from "vitest"
import {
  agentForProvider,
  composerModels,
  contextLabel,
  defaultComposerModel,
  fastModeFor,
  groupByProvider,
  providerLabel,
  thinkingLevels,
  thinkingLevelsFor,
  type ComposerModel,
  shortModelName,
} from "./composer-options"

function model(provider: string, modelId: string, reasoning = true): ComposerModel {
  return {
    provider,
    modelId,
    displayName: modelId,
    reasoning,
    maxContextWindowTokens: 200_000,
  }
}

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
  it("offers no levels to a model that does not reason", () => {
    expect(thinkingLevelsFor(model("x", "y", false))).toEqual([])
    expect(thinkingLevelsFor(undefined)).toEqual([])
  })

  it("offers levels to a reasoning model", () => {
    expect(thinkingLevelsFor(model("x", "y")).map((level) => level.value)).toEqual([
      "low",
      "medium",
      "high",
      "max",
    ])
  })
})

describe("fastModeFor", () => {
  it("offers Fast mode on Claude Opus 5 and not on other models", () => {
    expect(fastModeFor(model("anthropic", "claude-opus-5"))).toBe(true)
    expect(fastModeFor(model("anthropic", "claude-sonnet-5"))).toBe(false)
    expect(fastModeFor(model("openai", "claude-opus-5"))).toBe(false)
    expect(fastModeFor(undefined)).toBe(false)
  })
})

describe("a model's short name", () => {
  const model = (provider: string, displayName: string) => ({
    provider,
    modelId: displayName,
    displayName,
    reasoning: false,
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

  it("marks only the utmost thinking level, which the thinking control sets apart", () => {
    expect(
      thinkingLevels.filter((level) => level.utmost).map((level) => level.value),
    ).toEqual(["max"])
  })
})
