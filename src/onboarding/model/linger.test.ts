import { describe, expect, it } from "vitest"

import { parseLingerView } from "./linger"

describe("parseLingerView", () => {
  it("reads a confirming view", () => {
    expect(parseLingerView({ shown: "enabled", audit: "recorded" })).toEqual({
      shown: "enabled",
      audit: "recorded",
    })
  })

  it("rejects a missing audit even when the tag would be a claim", () => {
    expect(parseLingerView({ shown: "enabled" })).toBeUndefined()
  })

  it("rejects a tag this build does not name", () => {
    expect(parseLingerView({ shown: "constructor", audit: "not-required" })).toBeUndefined()
    expect(parseLingerView({ shown: "enabled", audit: "constructor" })).toBeUndefined()
  })

  it("does not read shown from the prototype", () => {
    const inherited = Object.create({ shown: "enabled", audit: "not-required" }) as unknown
    expect(parseLingerView(inherited)).toBeUndefined()
  })
})
