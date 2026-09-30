import { describe, expect, it } from "vitest"
import { characterLine } from "./character"

describe("a subagent's character", () => {
  it("is the same line for the same seed, every time", () => {
    expect(characterLine("xp-agent-sable")).toBe(characterLine("xp-agent-sable"))
  })

  it("differs across a crew", () => {
    const crew = ["wren", "juno", "orla", "pike", "sable", "tamsin"].map((name) =>
      characterLine(`xp-agent-${name}`),
    )
    expect(new Set(crew).size).toBeGreaterThan(3)
  })
})
