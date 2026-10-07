import { describe, expect, it } from "vitest"
import { subagentsWidgetState } from "./widget-state"
import type { SubagentRead } from "./ports"

const ready: SubagentRead = { kind: "ready", subagents: [], unreadable: [] }
const unread: SubagentRead = { kind: "unread" }

describe("the subagents widget's answer", () => {
  it("is off while the preview is off, ahead of everything else", () => {
    expect(
      subagentsWidgetState({
        sessionId: "a",
        preview: "off",
        listing: "absent",
        read: unread,
      }),
    ).toEqual({ kind: "off" })
  })

  it("is missing once the index is read and does not list the conversation", () => {
    expect(
      subagentsWidgetState({
        sessionId: "gone",
        preview: "on",
        listing: "absent",
        read: ready,
      }),
    ).toEqual({ kind: "missing" })
  })

  it("is unread while the index or the source has not read it", () => {
    expect(
      subagentsWidgetState({
        sessionId: "a",
        preview: "on",
        listing: "unread",
        read: ready,
      }),
    ).toEqual({ kind: "unread" })
    expect(
      subagentsWidgetState({
        sessionId: "a",
        preview: "on",
        listing: "listed",
        read: unread,
      }),
    ).toEqual({ kind: "unread" })
  })

  it("is ready, titled Subagents, for the conversation", () => {
    expect(
      subagentsWidgetState({
        sessionId: "a",
        preview: "on",
        listing: "listed",
        read: ready,
      }),
    ).toEqual({ kind: "ready", title: "Subagents", origin: "a" })
    expect(
      subagentsWidgetState({
        sessionId: "a",
        preview: "on",
        listing: "listed",
        read: { kind: "failed", failure: { kind: "unavailable" } },
      }).kind,
    ).toBe("ready")
  })
})
