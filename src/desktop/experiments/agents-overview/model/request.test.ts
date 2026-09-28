import { describe, expect, it } from "vitest"
import { parseExperimentFlag } from "./experiment-flag"
import { newer, requestOf } from "./request"
import type { Approval } from "../../../workspace/model/transcript"

const approval: Approval = {
  id: "run-tests",
  command: "cargo test",
  reason: "Runs the tests.",
}

/** A conversation as far as these rules read it: its ask and the source's count. */
const at = (revision: number, asked: Approval | null = approval) => ({
  approval: asked,
  revision,
})

describe("newer", () => {
  it("keeps whichever the source counted later, in either order", () => {
    const asked = at(2)
    const answered = at(3, null)
    expect(newer(asked, answered)).toBe(answered)
    expect(newer(answered, asked)).toBe(answered)
  })

  it("prefers what the window holds at the same count", () => {
    const held = at(2)
    expect(newer(held, at(2))).toBe(held)
  })

  it("takes whichever there is", () => {
    const only = at(1)
    expect(newer(undefined, only)).toBe(only)
    expect(newer(only, undefined)).toBe(only)
    expect(newer(undefined, undefined)).toBeUndefined()
  })
})

describe("requestOf", () => {
  it("is an approval while one is asked, a question while none is, and reading until read", () => {
    expect(requestOf({ approval })).toEqual({ kind: "approval", approval })
    expect(requestOf({ approval: null })).toEqual({ kind: "question" })
    expect(requestOf(undefined)).toEqual({ kind: "reading" })
  })
})

describe("parseExperimentFlag", () => {
  it("is on only when turned on", () => {
    expect(parseExperimentFlag("on")).toBe("on")
    expect(parseExperimentFlag("off")).toBe("off")
    expect(parseExperimentFlag(null)).toBe("off")
    expect(parseExperimentFlag("yes")).toBe("off")
    expect(parseExperimentFlag(1)).toBe("off")
  })
})
