import { describe, expect, it } from "vitest"
import type { Channel, SessionStatus, SessionSummary } from "./workspace-index"
import { fuzzy, switcherRows } from "./session-search"

const channels: Channel[] = [
  { id: "gateway", name: "gateway", sectionId: "labs", private: false, topic: "" },
  { id: "release", name: "release", sectionId: "starred", private: true, topic: "" },
]

function session(
  id: string,
  title: string,
  status: SessionStatus = "idle",
  updatedAt = 1,
): SessionSummary {
  return {
    id,
    channelId: "gateway",
    title,
    model: { provider: "openai", modelId: "gpt-6-astra" },
    status,
    startedAt: 0,
    updatedAt,
    preview: "",
    pinned: false,
    unread: false,
    revision: 1,
  }
}

describe("fuzzy matching", () => {
  it("matches characters in order, and nothing else", () => {
    expect(fuzzy("rtb", "Retry budget")?.hits).toEqual([0, 2, 6])
    expect(fuzzy("zz", "Retry budget")).toBeNull()
  })

  it("matches everything, with nothing highlighted, for an empty query", () => {
    expect(fuzzy("  ", "anything")).toEqual({ score: 0, hits: [] })
  })

  it("scores runs and word starts above scattered letters", () => {
    const run = fuzzy("bud", "Retry budget")!
    const scattered = fuzzy("bud", "abundant duty")!
    expect(run.score).toBeGreaterThan(scattered.score)
  })
})

describe("the switcher's rows", () => {
  const sessions = [
    session("retry", "Retry budget for reconnects", "needs-you", 5),
    session("logs", "Structured logs", "idle", 9),
    session("token", "Rotate the gateway token", "running", 7),
  ]

  it("offers a new session, then what waits, then the most recent, with nothing typed", () => {
    const rows = switcherRows({ query: "", sessions, channels, channelId: "gateway" })
    expect(
      rows.map((row) => [
        row.kind,
        row.group,
        row.kind === "session" ? row.session.id : "",
      ]),
    ).toEqual([
      ["new", "", ""],
      ["session", "Needs you", "retry"],
      ["session", "Recent", "logs"],
      ["session", "Recent", "token"],
    ])
  })

  it("offers matching sessions and channels, then starting with what was typed", () => {
    const rows = switcherRows({ query: "rel", sessions, channels, channelId: "gateway" })
    expect(rows.at(-1)).toEqual({
      kind: "new",
      group: "Start",
      channelId: "gateway",
      text: "rel",
    })
    expect(
      rows.some((row) => row.kind === "channel" && row.channel.id === "release"),
    ).toBe(true)
  })

  it("matches a session by its channel or agent when its title does not", () => {
    const rows = switcherRows({
      query: "codex",
      sessions,
      channels,
      channelId: "gateway",
    })
    expect(rows.filter((row) => row.kind === "session")).toHaveLength(3)
    expect(rows.every((row) => row.kind !== "session" || row.hits.length === 0)).toBe(
      true,
    )
  })
})
