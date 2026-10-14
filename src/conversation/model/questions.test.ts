import { describe, expect, it } from "vitest"
import { questionAnswerReady, questionChoices } from "./questions"

const fields = [
  { key: "where", required: true },
  { key: "note", required: false },
]

describe("questionAnswerReady", () => {
  it("holds an answer back until every required field has an option", () => {
    const chosen = new Map<string, readonly string[]>()
    const words = new Map<string, string>([["where", "in my own words"]])
    expect(questionAnswerReady(fields, chosen, words)).toBe(false)
    chosen.set("where", ["here"])
    expect(questionAnswerReady(fields, chosen, words)).toBe(true)
  })

  it("accepts an optional field answered only in the answerer's own words", () => {
    const chosen = new Map<string, readonly string[]>([["where", ["here"]]])
    const words = new Map<string, string>([["note", "  soon  "]])
    expect(questionAnswerReady(fields, chosen, words)).toBe(true)
    expect(questionChoices(fields, chosen, words)).toEqual([
      { key: "where", values: ["here"] },
      { key: "note", values: [], ownWords: "soon" },
    ])
  })

  it("omits a field nobody touched", () => {
    const chosen = new Map<string, readonly string[]>([["where", ["here"]]])
    expect(questionChoices(fields, chosen, new Map())).toEqual([
      { key: "where", values: ["here"] },
    ])
  })

  it("reads a field named like an object key from the map it was stored in", () => {
    const chosen = new Map<string, readonly string[]>([["toString", ["yes"]]])
    expect(
      questionAnswerReady([{ key: "toString", required: true }], chosen, new Map()),
    ).toBe(true)
    expect(
      questionChoices([{ key: "toString", required: true }], chosen, new Map()),
    ).toEqual([{ key: "toString", values: ["yes"] }])
  })
})
