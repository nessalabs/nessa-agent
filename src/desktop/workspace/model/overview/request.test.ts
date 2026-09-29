import { describe, expect, it } from "vitest"
import { requestOf } from "./request"
import type { Approval } from "../transcript"

const approval: Approval = {
  id: "run-tests",
  command: "cargo test",
  reason: "Runs the tests.",
}

describe("requestOf", () => {
  it("is an approval while one is asked, a question while none is, reading until read, and unreadable when the read failed", () => {
    expect(requestOf({ approval }, undefined)).toEqual({ kind: "approval", approval })
    expect(requestOf({ approval: null }, undefined)).toEqual({ kind: "question" })
    expect(requestOf(undefined, undefined)).toEqual({ kind: "reading" })
    expect(requestOf(undefined, "unavailable")).toEqual({
      kind: "unreadable",
      reason: "unavailable",
    })
  })

  it("shows what is held over an earlier failed read", () => {
    expect(requestOf({ approval }, "unavailable")).toEqual({ kind: "approval", approval })
  })
})
