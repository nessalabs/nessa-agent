import { describe, expect, it } from "vitest"
import type { Channel, SessionStatus, SessionSummary } from "./workspace-index"
import { fuzzy, switcherRows } from "./session-search"
import type { TerminalEntry } from "./terminal"

const channels: Channel[] = [
  { id: "gateway", name: "gateway", sectionId: "labs", private: false, topic: "" },
  { id: "release", name: "release", sectionId: "starred", private: true, topic: "" },
]

function terminal(
  id: string,
  title: string,
  over: Partial<TerminalEntry> = {},
): TerminalEntry {
  return {
    id,
    title,
    kind: "shell",
    channelId: "gateway",
    requestId: id,
    folder: "/w",
    shell: "/bin/zsh",
    runsOn: "here",
    startedAtMs: 1,
    state: "running",
    ...over,
  }
}

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

  const terminals = [
    terminal("old", "zsh", { startedAtMs: 1 }),
    terminal("new", "build watch", { startedAtMs: 9 }),
    terminal("run", "ls", { kind: "run", startedAtMs: 20 }),
  ]

  it("offers a new terminal and the newest shells, never a Run's, when the window has terminals", () => {
    const rows = switcherRows({
      query: "",
      sessions,
      channels,
      channelId: "gateway",
      terminals,
    })
    expect(
      rows
        .filter((row) => row.kind === "newTerminal" || row.kind === "terminal")
        .map((row) => [
          row.kind,
          row.group,
          row.kind === "terminal" ? row.terminal.id : "",
        ]),
    ).toEqual([
      ["newTerminal", "", ""],
      ["terminal", "Terminals", "new"],
      ["terminal", "Terminals", "old"],
    ])
    expect(rows[1]?.kind).toBe("newTerminal")
  })

  it("offers no terminal rows in a window without terminals", () => {
    const rows = switcherRows({ query: "", sessions, channels, channelId: "gateway" })
    expect(
      rows.some((row) => row.kind === "newTerminal" || row.kind === "terminal"),
    ).toBe(false)
  })

  it("matches a shell by its title, before starting with what was typed", () => {
    const rows = switcherRows({
      query: "watch",
      sessions,
      channels,
      channelId: "gateway",
      terminals,
    })
    const match = rows.find((row) => row.kind === "terminal")
    expect(match?.kind === "terminal" && match.terminal.id).toBe("new")
    expect(match?.kind === "terminal" && match.hits).toEqual([6, 7, 8, 9, 10])
    expect(rows.at(-1)?.kind).toBe("new")
  })
})
