import { describe, expect, it } from "vitest"

import { parseLingerView } from "./linger"

describe("parseLingerView", () => {
  it("reads the confirming view", () => {
    expect(parseLingerView({ shown: "enabled" })).toEqual({ shown: "enabled" })
  })

  it("rejects a tag that is not a screen", () => {
    expect(parseLingerView({ shown: "constructor" })).toBeUndefined()
    expect(parseLingerView({ shown: "waiting" })).toBeUndefined()
    expect(parseLingerView({})).toBeUndefined()
  })

  it("does not read a tag that lives only on the prototype", () => {
    const inherited = Object.create({ shown: "enabled" }) as unknown
    expect(parseLingerView(inherited)).toBeUndefined()
  })
})
