import { describe, expect, it } from "vitest"
import { defaultFilter } from "../../model/overview/filter"
import { rememberedFilter } from "./remembered-filter"

function memory(): Pick<Storage, "getItem" | "setItem"> & { held: Map<string, string> } {
  const held = new Map<string, string>()
  return {
    held,
    getItem: (key) => held.get(key) ?? null,
    setItem: (key, value) => void held.set(key, value),
  }
}

describe("rememberedFilter", () => {
  it("reads back what it wrote, and the default when nothing is kept", () => {
    const storage = memory()
    const filter = rememberedFilter(() => storage)
    expect(filter.read()).toEqual(defaultFilter)
    filter.write({ scope: "all", range: "week", tags: [] })
    expect(filter.read()).toEqual({ scope: "all", range: "week", tags: [] })
  })

  it("survives storage that refuses: the default is read, and a write is let go", () => {
    const filter = rememberedFilter(() => {
      throw new Error("no storage")
    })
    expect(filter.read()).toEqual(defaultFilter)
    expect(() => filter.write({ ...defaultFilter, scope: "all" })).not.toThrow()
  })
})
