import { describe, expect, it } from "vitest"
import type { Message } from "./transcript"
import {
  compactActivityLabel,
  liveActivity,
  parseTranscriptView,
} from "./transcript-view"

describe("transcript views", () => {
  it("reads a stored view, falling back to detailed for anything unknown", () => {
    expect(parseTranscriptView("compact")).toBe("compact")
    expect(parseTranscriptView("detailed")).toBe("detailed")
    expect(parseTranscriptView(null)).toBe("detailed")
    expect(parseTranscriptView("verbose")).toBe("detailed")
  })

  it("says only that the agent is working in the compact view, live run or not", () => {
    const activity = { label: "Read dev.log", since: 5 }
    const live: Message = {
      id: "m",
      role: "agent",
      at: 1,
      working: true,
      parts: [{ kind: "step", step: "read", label: "Read", id: "t", status: "running" }],
    }
    // Detailed leaves it to the live run's trace.
    expect(liveActivity("detailed", [live], activity)).toBeNull()
    expect(liveActivity("detailed", [], activity)).toBe(activity)
    // Compact draws no trace, so the row shows, naming no call.
    expect(liveActivity("compact", [live], activity)).toEqual({
      label: compactActivityLabel,
      since: 5,
    })
    expect(liveActivity("compact", [live], null)).toBeNull()
  })
})
