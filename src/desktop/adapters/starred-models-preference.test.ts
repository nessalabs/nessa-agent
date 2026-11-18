import { describe, expect, it } from "vitest"
import { parseStarredModels, starredModelsLimit } from "./starred-models-preference"

describe("parseStarredModels", () => {
  it("reads the starred models in the order they were starred", () => {
    const stored = JSON.stringify([
      { provider: "openai", modelId: "gpt-6-astra" },
      { provider: "anthropic", modelId: "claude-opus-5" },
    ])
    expect(parseStarredModels(stored)).toEqual([
      { provider: "openai", modelId: "gpt-6-astra" },
      { provider: "anthropic", modelId: "claude-opus-5" },
    ])
  })

  it("starts empty when nothing, or nothing it can read, is stored", () => {
    expect(parseStarredModels(null)).toEqual([])
    expect(parseStarredModels(undefined)).toEqual([])
    expect(parseStarredModels("not json")).toEqual([])
    expect(parseStarredModels(JSON.stringify({ provider: "openai" }))).toEqual([])
  })

  it("drops a bad entry or a repeat without losing the rest", () => {
    const stored = JSON.stringify([
      { provider: "openai", modelId: "gpt-6-astra" },
      { provider: "openai" },
      "claude-opus-5",
      null,
      { provider: "openai", modelId: 7 },
      { provider: "openai", modelId: "gpt-6-astra" },
      { provider: "anthropic", modelId: "claude-opus-5", extra: true },
    ])
    expect(parseStarredModels(stored)).toEqual([
      { provider: "openai", modelId: "gpt-6-astra" },
      { provider: "anthropic", modelId: "claude-opus-5" },
    ])
  })

  it("keeps at most the limit, the newest", () => {
    const stored = JSON.stringify(
      Array.from({ length: starredModelsLimit + 5 }, (_, index) => ({
        provider: "opencode",
        modelId: `model-${index}`,
      })),
    )
    expect(parseStarredModels(stored)).toHaveLength(starredModelsLimit)
    expect(parseStarredModels(stored).at(-1)?.modelId).toBe(
      `model-${starredModelsLimit + 4}`,
    )
  })
})
