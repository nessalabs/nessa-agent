import { describe, expect, it } from "vitest"
import type { Message, Part, Transcript } from "../transcript"
import { peekOf } from "./peek"

const step = (label: string): Part => ({ kind: "step", step: "read", label })
const text = (words: string): Part => ({ kind: "text", text: words })

function message(role: Message["role"], at: number, ...parts: Part[]): Message {
  return { id: `${role}-${at}`, role, at, parts }
}

function conversation(...messages: Message[]): Transcript {
  return { sessionId: "s", messages, activity: null, approval: null, revision: 1 }
}

describe("peekOf", () => {
  it("drops widgets before counting a turn's parts, keeping messages without one as they are", () => {
    const widget: Part = { kind: "widget", widget: { plugin: "mcp:charts", id: "call" } }
    const asked = message("user", 1, text("Chart it."))
    const drawn = message("agent", 2, step("a"), widget, text("Here."), widget)
    const plain = message("agent", 3, step("b"))
    const peek = peekOf(conversation(asked, drawn, plain), 3)
    expect(peek.since[0].parts).toEqual([step("a"), text("Here.")])
    expect(peek.since[0].id).toBe(drawn.id)
    expect(peek.since[1]).toBe(plain)
    expect(peek.earlier).toBe(false)
    expect(peekOf(conversation(asked, drawn, plain), 2).earlier).toBe(true)
  })

  it("draws no message of widgets alone, so a turn of only widgets draws nothing", () => {
    const widget: Part = { kind: "widget", widget: { plugin: "mcp:charts", id: "call" } }
    const only = message("agent", 1, widget, widget)
    expect(peekOf(conversation(only)).since).toEqual([])
    const asked = message("user", 2, text("Again."))
    const words = message("agent", 3, text("Done."))
    expect(peekOf(conversation(asked, only, words)).since).toEqual([words])
    // A reply with no parts yet — just begun streaming — is kept as it always was.
    const begun = message("agent", 4)
    expect(peekOf(conversation(asked, begun)).since).toEqual([begun])
  })

  it("tells the turn from the person's latest message, then everything the agent did since, in order", () => {
    const first = message("user", 1, text("Fix it."))
    const earlier = message("agent", 2, step("old"), text("Fixed."))
    const asked = message("user", 3, text("Now test it."))
    const steps = message("agent", 4, step("a"), step("b"), text("Running the tests."))
    const words = message("agent", 5, text("They pass."), step("c"))
    const peek = peekOf(conversation(first, earlier, asked, steps, words))
    expect(peek.asked).toBe(asked)
    expect(peek.since).toEqual([steps, words])
    expect(peek.earlier).toBe(false)
  })

  it("draws the latest parts of a long turn and says there is more above, the oldest drawn cut to its latest parts", () => {
    const parts = (from: number, count: number) =>
      Array.from({ length: count }, (_, index) => step(`s${from + index}`))
    const asked = message("user", 1, text("Go."))
    const early = message("agent", 2, ...parts(0, 10))
    const middle = message("agent", 3, ...parts(10, 20))
    const latest = message("agent", 4, ...parts(30, 10))
    const peek = peekOf(conversation(asked, early, middle, latest), 24)
    expect(peek.asked).toBe(asked)
    expect(peek.earlier).toBe(true)
    expect(peek.since).toHaveLength(2)
    // The latest message as it is; the one before it cut to its last 14 parts, under its own id.
    expect(peek.since[1]).toBe(latest)
    expect(peek.since[0].id).toBe(middle.id)
    expect(peek.since[0].parts).toEqual(middle.parts.slice(-14))
    expect(peek.since.flatMap((each) => each.parts)).toHaveLength(24)
  })

  it("draws a single message longer than the bound from its latest parts", () => {
    const many = Array.from({ length: 40 }, (_, index) => step(`s${index}`))
    const peek = peekOf(
      conversation(message("user", 1, text("Go.")), message("agent", 2, ...many)),
      24,
    )
    expect(peek.since[0].parts).toEqual(many.slice(-24))
    expect(peek.earlier).toBe(true)
  })

  it("draws a turn that fits whole, and says nothing is above", () => {
    const whole = message("agent", 2, ...Array.from({ length: 24 }, () => step("s")))
    const peek = peekOf(conversation(message("user", 1, text("Go.")), whole), 24)
    expect(peek.since).toEqual([whole])
    expect(peek.since[0]).toBe(whole)
    expect(peek.earlier).toBe(false)
  })

  it("bounds the conversation before the person has written, as a turn", () => {
    const opening = Array.from({ length: 30 }, (_, index) =>
      message("agent", index, text(`${index}`)),
    )
    const peek = peekOf(conversation(...opening), 24)
    expect(peek.asked).toBeNull()
    expect(peek.since).toEqual(opening.slice(-24))
    expect(peek.earlier).toBe(true)
  })

  it("tells the whole conversation before the person has written", () => {
    const opening = message("agent", 1, text("Hello."))
    const peek = peekOf(conversation(opening))
    expect(peek.asked).toBeNull()
    expect(peek.since).toEqual([opening])
    expect(peek.earlier).toBe(false)
  })

  it("ends on the person's message while the agent has not answered it", () => {
    const asked = message("user", 3, text("And now?"))
    const peek = peekOf(
      conversation(
        message("user", 1, text("Hi.")),
        message("agent", 2, text("Hi.")),
        asked,
      ),
    )
    expect(peek.asked).toBe(asked)
    expect(peek.since).toEqual([])
  })

  it("tells nothing of an empty conversation, and carries what the agent is doing", () => {
    const activity = { label: "Running the tests", since: 5 }
    const peek = peekOf({ ...conversation(), activity })
    expect(peek.asked).toBeNull()
    expect(peek.since).toEqual([])
    expect(peek.activity).toBe(activity)
  })
})
