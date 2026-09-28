import { describe, expect, it } from "vitest"
import type { Message, Part, Transcript } from "../../../workspace/model/transcript"
import { peekOf, peekSteps } from "./peek"

const step = (label: string): Part => ({ kind: "step", step: "read", label })
const text = (words: string): Part => ({ kind: "text", text: words })

function message(role: Message["role"], at: number, ...parts: Part[]): Message {
  return { id: `${role}-${at}`, role, at, parts }
}

function conversation(...messages: Message[]): Transcript {
  return { sessionId: "s", messages, activity: null, approval: null, revision: 1 }
}

describe("peekOf", () => {
  it("shows this turn's steps — since the person last wrote — and the agent's last words", () => {
    const peek = peekOf(
      conversation(
        message("user", 1, text("Fix it.")),
        message("agent", 2, step("old"), text("Done before.")),
        message("user", 3, text("Again.")),
        message("agent", 4, step("a"), step("b"), text("Looking.")),
        message("agent", 5, step("c")),
      ),
    )
    expect(peek.steps.map((each) => each.label)).toEqual(["a", "b", "c"])
    expect(peek.earlier).toBe(0)
    expect(peek.said).toEqual({ text: "Looking.", at: 4 })
  })

  it("keeps the last few steps and counts the ones before", () => {
    const many = Array.from({ length: peekSteps + 3 }, (_, index) => step(`s${index}`))
    const peek = peekOf(conversation(message("agent", 1, ...many)))
    expect(peek.steps).toHaveLength(peekSteps)
    expect(peek.steps[0].label).toBe("s3")
    expect(peek.earlier).toBe(3)
  })

  it("has no steps once the person has written and the agent has not answered", () => {
    const peek = peekOf(
      conversation(
        message("agent", 1, step("a"), text("Ready.")),
        message("user", 2, text("Go on.")),
      ),
    )
    expect(peek.steps).toEqual([])
    // What it said last still stands.
    expect(peek.said?.text).toBe("Ready.")
  })

  it("says nothing for a reply that has not begun to stream", () => {
    const peek = peekOf(conversation(message("agent", 1, step("a"), text(""))))
    expect(peek.said).toBeNull()
  })

  it("carries what the agent is doing now", () => {
    const activity = { label: "Editing panes.tsx", since: 10 }
    expect(peekOf({ ...conversation(), activity }).activity).toBe(activity)
  })
})
