import { describe, expect, it } from "vitest"
import { afterAnswer, answerPause, stepFrom, takesAnswerKey } from "./walk"

describe("stepFrom", () => {
  const order = ["a", "b", "c"]

  it("walks down and up, held at the ends", () => {
    expect(stepFrom(order, "a", "next")).toBe("b")
    expect(stepFrom(order, "c", "next")).toBe("c")
    expect(stepFrom(order, "b", "previous")).toBe("a")
    expect(stepFrom(order, "a", "previous")).toBe("a")
  })

  it("jumps to either end", () => {
    expect(stepFrom(order, "b", "first")).toBe("a")
    expect(stepFrom(order, "b", "last")).toBe("c")
  })

  it("starts from the first when its place went away, or nothing had it", () => {
    expect(stepFrom(order, "gone", "next")).toBe("a")
    expect(stepFrom(order, null, "previous")).toBe("a")
  })

  it("lands nowhere in an empty list", () => {
    expect(stepFrom([], "a", "next")).toBeNull()
    expect(stepFrom([], null, "first")).toBeNull()
  })
})

describe("afterAnswer", () => {
  const none = new Set<string>()

  it("moves on to the next request below", () => {
    expect(afterAnswer(["a", "b", "c"], "b", none)).toBe("c")
  })

  it("goes back up from the last request", () => {
    expect(afterAnswer(["a", "b", "c"], "c", none)).toBe("b")
  })

  it("passes over requests already answered and settling", () => {
    expect(afterAnswer(["a", "b", "c", "d"], "b", new Set(["c"]))).toBe("d")
    expect(afterAnswer(["a", "b", "c"], "c", new Set(["b"]))).toBe("a")
  })

  it("rests once no request is left, rather than leaving for another group", () => {
    expect(afterAnswer(["a", "b"], "b", new Set(["a"]))).toBeNull()
    expect(afterAnswer(["a"], "a", none)).toBeNull()
  })

  it("takes the first open request when the answered one is not listed", () => {
    expect(afterAnswer(["a", "b"], "gone", new Set(["a"]))).toBe("b")
  })
})

describe("takesAnswerKey", () => {
  it("never takes a held key's repeat: one press answers one request", () => {
    expect(takesAnswerKey({ repeat: true, at: 10_000 }, null)).toBe(false)
    expect(takesAnswerKey({ repeat: false, at: 10_000 }, null)).toBe(true)
  })

  it("waits a moment after the keyboard moves on by answering before a press answers again", () => {
    expect(takesAnswerKey({ repeat: false, at: 1080 }, 1000)).toBe(false)
    expect(takesAnswerKey({ repeat: false, at: 1000 + answerPause - 1 }, 1000)).toBe(
      false,
    )
    expect(takesAnswerKey({ repeat: false, at: 1000 + answerPause }, 1000)).toBe(true)
    expect(takesAnswerKey({ repeat: true, at: 5000 }, 1000)).toBe(false)
  })
})
