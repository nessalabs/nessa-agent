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

const { defaultModel } = await import("./overview")
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
