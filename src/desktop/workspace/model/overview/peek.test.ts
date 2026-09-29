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
  it("tells the turn from the person's latest message, then everything the agent did since, in order", () => {
    const first = message("user", 1, text("Fix it."))
    const earlier = message("agent", 2, step("old"), text("Fixed."))
    const asked = message("user", 3, text("Now test it."))
    const steps = message("agent", 4, step("a"), step("b"), text("Running the tests."))
    const words = message("agent", 5, text("They pass."), step("c"))
    const peek = peekOf(conversation(first, earlier, asked, steps, words))
    expect(peek.asked).toBe(asked)
    expect(peek.since).toEqual([steps, words])
  })

  it("keeps every step of a long turn, none counted away", () => {
    const many = Array.from({ length: 40 }, (_, index) => step(`s${index}`))
    const peek = peekOf(
      conversation(message("user", 1, text("Go.")), message("agent", 2, ...many)),
    )
    expect(peek.since[0].parts).toHaveLength(40)
  })

  it("tells the whole conversation before the person has written", () => {
    const opening = message("agent", 1, text("Hello."))
    const peek = peekOf(conversation(opening))
    expect(peek.asked).toBeNull()
    expect(peek.since).toEqual([opening])
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
