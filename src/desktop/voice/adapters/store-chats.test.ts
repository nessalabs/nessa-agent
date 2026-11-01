import { describe, expect, it } from "vitest"
import type { Transcript, TurnMessage } from "../../workspace/model/transcript"
import { recentTurnsOf, turnState } from "./store-chats"

function turn(
  role: "user" | "agent",
  text: string,
  extra: Partial<TurnMessage> = {},
): TurnMessage {
  return { id: `${role}-${text}`, role, at: 0, parts: [{ kind: "text", text }], ...extra }
}

function transcript(...messages: TurnMessage[]): Transcript {
  return { sessionId: "s", messages, activity: null, approvals: [], revision: 1 }
}

describe("turnState", () => {
  const sent = "11111111-1111-4111-8111-111111111111"
  const other = "22222222-2222-4222-8222-222222222222"

  it("is the turn's reply once it is no longer being written", () => {
    const writing = transcript(
      turn("user", "go", { id: sent }),
      turn("agent", "Half", { id: `${sent}/reply`, working: true }),
    )
    expect(turnState(writing, sent)).toEqual({ kind: "open" })
    const done = transcript(
      turn("user", "go", { id: sent }),
      turn("agent", "All done.", { id: `${sent}/reply` }),
    )
    expect(turnState(done, sent)).toEqual({
      kind: "ended",
      reply: { text: "All done.", ended: "finished" },
    })
  })

  it("is never another turn's, whatever its text", () => {
    const chat = transcript(
      turn("user", "yes", { id: other }),
      turn("agent", "Old answer.", { id: `${other}/reply` }),
      { ...turn("user", "yes", { id: sent }) },
    )
    const working = { ...chat, activity: { label: "Thinking", since: 0 } }
    expect(turnState(working, sent)).toEqual({ kind: "open" })
  })

  it("says a stopped or failed turn ended so, reply or none", () => {
    const stopped = transcript(
      turn("user", "go", { id: sent }),
      turn("agent", "Partly", { id: `${sent}/reply`, terminal: { status: "cancelled" } }),
    )
    expect(turnState(stopped, sent)).toEqual({
      kind: "ended",
      reply: { text: "Partly", ended: "stopped" },
    })
    const failed = transcript(
      turn("user", "go", { id: sent, terminal: { status: "failed" }, executionId: sent }),
    )
    expect(turnState(failed, sent)).toEqual({
      kind: "ended",
      reply: { text: "", ended: "failed" },
    })
  })

  it("is absent while missing, and open while waiting, queued or running", () => {
    expect(turnState(transcript(), sent)).toEqual({ kind: "absent" })
    const queued = {
      ...transcript(turn("user", "go", { id: sent })),
      activity: { label: "Thinking", since: 0 },
    }
    expect(turnState(queued, sent)).toEqual({ kind: "open" })
    // Held waiting for an agent with nothing at work: still not a finish.
    const held = transcript(turn("user", "go", { id: sent, waiting: true }))
    expect(turnState(held, sent)).toEqual({ kind: "open" })
  })

  it("is an empty finish once its message stands answered by nothing and the chat is quiet", () => {
    expect(turnState(transcript(turn("user", "go", { id: sent })), sent)).toEqual({
      kind: "ended",
      reply: { text: "", ended: "finished" },
    })
  })
})

describe("recentTurnsOf", () => {
  it("keeps the latest six turns, one line each, cut to 600 characters", () => {
    const messages = Array.from({ length: 8 }, (_, n) =>
      turn(n % 2 ? "agent" : "user", `turn ${n}\nsecond line`),
    )
    messages.push(turn("agent", "x".repeat(700)))
    const lines = recentTurnsOf(transcript(...messages)).split("\n")
    expect(lines).toHaveLength(6)
    expect(lines[0]).toBe("Agent: turn 3 second line")
    expect(lines.at(-1)).toBe(`Agent: ${"x".repeat(600)}`)
  })

  it("is empty for a chat with nothing said", () => {
    expect(recentTurnsOf(undefined)).toBe("")
  })
})
