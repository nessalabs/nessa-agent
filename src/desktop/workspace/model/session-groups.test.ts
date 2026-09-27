import { describe, expect, it } from "vitest"
import type { SessionStatus, SessionSummary } from "./organisation"
import {
  branchCap,
  branchSessions,
  channelActivity,
  groupByStatus,
  inView,
  matchesSearch,
  mostPressing,
  statusCounts,
} from "./session-groups"

function session(
  id: string,
  updatedAt: number,
  status: SessionStatus = "idle",
  extra: Partial<SessionSummary> = {},
): SessionSummary {
  return {
    id,
    channelId: "desktop-app",
    title: `Session ${id}`,
    model: { provider: "anthropic", modelId: "claude-opus-5" },
    status,
    startedAt: updatedAt,
    updatedAt,
    preview: "",
    pinned: false,
    unread: false,
    revision: 1,
    ...extra,
  }
}

describe("grouping by status", () => {
  it("orders the groups Needs you, Running, Earlier, newest first in each, and leaves empty ones out", () => {
    const groups = groupByStatus([
      session("old", 1),
      session("new", 5),
      session("run", 3, "running"),
      session("wait", 2, "needs-you"),
    ])
    expect(groups.map((group) => [group.label, group.sessions.map((s) => s.id)])).toEqual(
      [
        ["Needs you", ["wait"]],
        ["Running", ["run"]],
        ["Earlier", ["new", "old"]],
      ],
    )
    expect(groupByStatus([session("a", 1)]).map((group) => group.status)).toEqual([
      "idle",
    ])
  })
})

describe("views and search", () => {
  it("shows a channel's sessions, or every session in one state", () => {
    const a = session("a", 1, "running")
    const b = session("b", 1, "idle", { channelId: "sdk" })
    expect(inView(a, { kind: "channel", channelId: "desktop-app" })).toBe(true)
    expect(inView(b, { kind: "channel", channelId: "desktop-app" })).toBe(false)
    expect(inView(a, { kind: "status", status: "running" })).toBe(true)
    expect(inView(b, { kind: "status", status: "running" })).toBe(false)
  })

  it("searches title, preview, model and agent, ignoring case and spacing", () => {
    const a = session("a", 1, "idle", {
      title: "Rain density",
      preview: "Tuned the far layer",
    })
    expect(matchesSearch(a, "  ")).toBe(true)
    expect(matchesSearch(a, "RAIN")).toBe(true)
    expect(matchesSearch(a, "far layer")).toBe(true)
    expect(matchesSearch(a, "opus")).toBe(true)
    expect(matchesSearch(a, "claude")).toBe(true)
    expect(matchesSearch(a, "codex")).toBe(false)
  })
})

describe("the most pressing session", () => {
  it("is one waiting, then one running, then the newest", () => {
    expect(
      mostPressing([
        session("new", 9),
        session("run", 2, "running"),
        session("wait", 1, "needs-you"),
      ])?.id,
    ).toBe("wait")
    expect(mostPressing([session("new", 9), session("run", 2, "running")])?.id).toBe(
      "run",
    )
    expect(mostPressing([session("old", 1), session("new", 9)])?.id).toBe("new")
    expect(mostPressing([])).toBeUndefined()
  })
})

describe("a channel's branch", () => {
  const many = [1, 2, 3, 4, 5, 6].map((at) => session(`s${at}`, at))

  it("discloses the newest few", () => {
    const { visible, hidden } = branchSessions(many, { showAll: false, shown: new Set() })
    expect(visible.map((s) => s.id)).toEqual(["s6", "s5", "s4"])
    expect(visible).toHaveLength(branchCap)
    expect(hidden).toBe(3)
  })

  it("never hides a session on screen or one waiting on the person", () => {
    const waiting = [...many, session("wait", 0, "needs-you")]
    const { visible, hidden } = branchSessions(waiting, {
      showAll: false,
      shown: new Set(["s1"]),
    })
    expect(visible.map((s) => s.id)).toEqual(["s6", "s5", "s4", "s1", "wait"])
    expect(hidden).toBe(2)
  })

  it("shows every session when asked", () => {
    const { visible, hidden } = branchSessions(many, { showAll: true, shown: new Set() })
    expect(visible).toHaveLength(6)
    expect(hidden).toBe(0)
  })

  it("summarises what waits, runs and is unread", () => {
    expect(
      channelActivity([
        session("a", 1, "needs-you"),
        session("b", 1, "needs-you", { unread: true }),
        session("c", 1, "running"),
      ]),
    ).toEqual({ waiting: 2, running: true, unread: true })
    expect(channelActivity([])).toEqual({ waiting: 0, running: false, unread: false })
  })

  it("counts every session waiting and running", () => {
    expect(
      statusCounts([
        session("a", 1, "needs-you"),
        session("b", 1, "running"),
        session("c", 1),
      ]),
    ).toEqual({ needsYou: 1, running: 1 })
  })
})
