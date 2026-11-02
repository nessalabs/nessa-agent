import { describe, expect, it } from "vitest"
import { tidy } from "./tidy"

describe("tidy", () => {
  it("drops fillers and keeps every other word", () => {
    expect(tidy("um, can you uh check the tests")).toBe("Can you check the tests")
    expect(tidy("Umm so erm look at the build, hmm.")).toBe("So look at the build.")
  })

  it("drops a word said twice in a row", () => {
    expect(tidy("open the the settings page")).toBe("Open the settings page")
    expect(tidy("I I think so")).toBe("I think so")
  })

  it("keeps a sentence's end when the filler held it", () => {
    expect(tidy("fix the bug um.")).toBe("Fix the bug.")
  })

  it("closes spaces and changes nothing it does not need to", () => {
    expect(tidy("  Run   pnpm test , please ")).toBe("Run pnpm test, please")
    expect(tidy("Is PR 1220 merged?")).toBe("Is PR 1220 merged?")
    expect(tidy("Humming is fine; summary too.")).toBe("Humming is fine; summary too.")
  })

  it("is empty for nothing but fillers", () => {
    expect(tidy("um uh")).toBe("")
    expect(tidy("")).toBe("")
  })
})
