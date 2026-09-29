import { describe, expect, it } from "vitest"
import { knownToSource, supersedes } from "./revision"

describe("which replacement is newer", () => {
  it("takes one newer than what is held, or anything when nothing is", () => {
    expect(supersedes({ revision: 3 }, undefined)).toBe(true)
    expect(supersedes({ revision: 3 }, { revision: 2 })).toBe(true)
    expect(supersedes({ revision: 2 }, { revision: 3 })).toBe(false)
  })

  it("changes nothing for the same revision again", () => {
    expect(supersedes({ revision: 3 }, { revision: 3 })).toBe(false)
  })

  it("tells the window's own from what the source has spoken of", () => {
    expect(knownToSource({ revision: 0 })).toBe(false)
    expect(knownToSource({ revision: 1 })).toBe(true)
  })
})
