import { describe, expect, it } from "vitest"
import type {
  SessionStatus,
  SessionSummary,
} from "../../../workspace/model/workspace-index"
import {
  defaultFilter,
  filterLabel,
  filtered,
  parseFilter,
  rangeStart,
  serializeFilter,
  type AgentsFilter,
} from "./filter"

const day = 24 * 60 * 60 * 1000
// Noon, local time, so "today" and the day before are unambiguous.
const now = new Date(2026, 8, 27, 12).getTime()

function session(
  id: string,
  ago: number,
  status: SessionStatus = "idle",
): SessionSummary {
  return {
    id,
    channelId: "c",
    title: id,
    model: { provider: "anthropic", modelId: "claude-opus-5" },
    status,
    startedAt: now - ago - 10,
    updatedAt: now - ago,
    preview: "",
    pinned: false,
    unread: false,
    revision: 1,
  }
}

const noTags = () => []
const ids = (sessions: readonly SessionSummary[]) => sessions.map((each) => each.id)

describe("filtered", () => {
  const sessions = [
    session("ask-old", 40 * day, "needs-you"),
    session("run-today", 1000, "running"),
    session("run-old", 10 * day, "running"),
    session("idle-today", 2000),
    session("idle-week", 3 * day),
    session("idle-month", 20 * day),
    session("idle-ancient", 90 * day),
  ]
  const at = (filter: Partial<AgentsFilter>) =>
    ids(filtered(sessions, { ...defaultFilter, ...filter }, now, noTags))

  it("opens on what is going on: waiting on the person, or working", () => {
    expect(at({})).toEqual(["ask-old", "run-today", "run-old"])
  })

  it("lists every session under All", () => {
    expect(at({ scope: "all" })).toHaveLength(sessions.length)
  })

  it("keeps to a span of time by when each last moved, but never hides a request", () => {
    expect(at({ scope: "all", range: "today" })).toEqual([
      "ask-old",
      "run-today",
      "idle-today",
    ])
    expect(at({ scope: "all", range: "week" })).toEqual([
      "ask-old",
      "run-today",
      "idle-today",
      "idle-week",
    ])
    expect(at({ scope: "all", range: "month" })).toEqual([
      "ask-old",
      "run-today",
      "run-old",
      "idle-today",
      "idle-week",
      "idle-month",
    ])
  })

  it("lets through only sessions carrying a chosen tag", () => {
    const tagsOf = (each: SessionSummary) => (each.id === "run-today" ? ["ui"] : [])
    expect(
      ids(
        filtered(sessions, { ...defaultFilter, scope: "all", tags: ["ui"] }, now, tagsOf),
      ),
    ).toEqual(["run-today"])
    // With no tags carried, a tag filter lets nothing through: nothing is faked.
    expect(
      ids(filtered(sessions, { ...defaultFilter, tags: ["ui"] }, now, noTags)),
    ).toEqual([])
  })
})

describe("rangeStart", () => {
  it("counts days from the start of today, in local time", () => {
    const midnight = new Date(2026, 8, 27).getTime()
    expect(rangeStart("today", now)).toBe(midnight)
    expect(rangeStart("week", now)).toBe(midnight - 6 * day)
    expect(rangeStart("month", now)).toBe(midnight - 29 * day)
    expect(rangeStart("any", now)).toBe(-Infinity)
  })
})

describe("parseFilter", () => {
  it("reads back what it wrote", () => {
    const filter: AgentsFilter = { scope: "all", range: "week", tags: ["ui"] }
    expect(parseFilter(serializeFilter(filter))).toEqual(filter)
  })

  it("falls back field by field on anything it does not know", () => {
    expect(parseFilter(null)).toEqual(defaultFilter)
    expect(parseFilter("not json")).toEqual(defaultFilter)
    expect(parseFilter("[]")).toEqual({ ...defaultFilter })
    expect(parseFilter(JSON.stringify({ scope: "all", range: "decade" }))).toEqual({
      ...defaultFilter,
      scope: "all",
    })
    expect(parseFilter(JSON.stringify({ tags: ["ui", 3, null] })).tags).toEqual(["ui"])
  })

  it("reads only what the stored object itself holds", () => {
    expect(parseFilter('{"__proto__": {"scope": "all"}}')).toEqual(defaultFilter)
    expect(parseFilter(JSON.stringify({ constructor: "x" }))).toEqual(defaultFilter)
  })
})

describe("filterLabel", () => {
  it("names the scope, and the span when one is chosen", () => {
    expect(filterLabel(defaultFilter)).toBe("Ongoing")
    expect(filterLabel({ ...defaultFilter, scope: "all", range: "week" })).toBe(
      "All · Last 7 days",
    )
  })
})
