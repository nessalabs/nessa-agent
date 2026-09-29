import { describe, expect, it } from "vitest"
import type { SessionStatus, SessionSummary } from "../workspace-index"
import {
  agentsGlance,
  glanceCounts,
  groupOf,
  readingOrder,
  sameGlance,
  type GlanceContext,
} from "./agents-glance"
import { defaultFilter } from "./filter"

function session(
  id: string,
  updatedAt: number,
  status: SessionStatus = "idle",
  unread = false,
): SessionSummary {
  return {
    id,
    channelId: "desktop",
    title: `Session ${id}`,
    model: { provider: "anthropic", modelId: "claude-opus-5" },
    status,
    startedAt: updatedAt - 10,
    updatedAt,
    preview: "",
    pinned: false,
    unread,
    revision: 1,
  }
}

const noTags = () => []
const all: GlanceContext = {
  filter: { ...defaultFilter, scope: "all" },
  now: 100,
  tagsOf: noTags,
}
const ongoing: GlanceContext = { filter: defaultFilter, now: 100, tagsOf: noTags }

describe("agentsGlance", () => {
  const sessions = [
    session("old-ask", 10, "needs-you"),
    session("new-ask", 30, "needs-you"),
    session("run", 20, "running"),
    session("run-later", 40, "running"),
    session("done", 25, "idle", true),
    session("seen", 50),
  ]

  it("lists what waits on the person first, then what works, what finished unseen, and the rest, each newest first", () => {
    const glance = agentsGlance(sessions, [], all)
    expect(glance.needsYou).toEqual(["new-ask", "old-ask"])
    expect(glance.working).toEqual(["run-later", "run"])
    expect(glance.finished).toEqual(["done"])
    expect(glance.earlier).toEqual(["seen"])
    expect(glance.hidden).toBe(0)
    expect(glance.counts).toEqual({ needsYou: 2, working: 2, finished: 1, earlier: 1 })
    expect(readingOrder(glance)).toEqual([
      "new-ask",
      "old-ask",
      "run-later",
      "run",
      "done",
      "seen",
    ])
  })

  it("keeps idle sessions out while only what is ongoing is asked for, and counts them", () => {
    const glance = agentsGlance(sessions, [], ongoing)
    expect(glance.finished).toEqual([])
    expect(glance.earlier).toEqual([])
    expect(glance.hidden).toBe(2)
    expect(glance.needsYou).toHaveLength(2)
  })

  it("holds an answered request where it stood, though the source has moved it on", () => {
    const glance = agentsGlance(
      [
        session("a", 30, "needs-you"),
        // Answered: the source says it runs now, and moved it to the top by time.
        session("b", 99, "running"),
        session("c", 10, "needs-you"),
      ],
      [{ sessionId: "b", updatedAt: 20 }],
      ongoing,
    )
    expect(glance.needsYou).toEqual(["a", "b", "c"])
    expect(glance.working).toEqual([])
    // The header counts only what still waits.
    expect(glance.counts.needsYou).toBe(2)
  })

  it("keeps a held request whatever the filter says, so its settle finishes", () => {
    const glance = agentsGlance(
      [session("b", 99, "idle")],
      [{ sessionId: "b", updatedAt: 20 }],
      ongoing,
    )
    expect(glance.needsYou).toEqual(["b"])
    expect(glance.hidden).toBe(0)
  })

  it("keeps listing the session being looked at when it leaves the filter, until the person moves on", () => {
    const finishing = [session("a", 30, "needs-you"), session("b", 40, "idle", true)]
    expect(agentsGlance(finishing, [], { ...ongoing, looking: "b" }).finished).toEqual([
      "b",
    ])
    expect(agentsGlance(finishing, [], ongoing).finished).toEqual([])
  })

  it("lets a held request go into its new group once released", () => {
    const glance = agentsGlance(
      [session("a", 30, "needs-you"), session("b", 99, "running")],
      [],
      ongoing,
    )
    expect(glance.needsYou).toEqual(["a"])
    expect(glance.working).toEqual(["b"])
  })

  it("forgets a held request whose session is gone", () => {
    const glance = agentsGlance(
      [session("a", 30, "needs-you")],
      [{ sessionId: "archived", updatedAt: 40 }],
      ongoing,
    )
    expect(glance.needsYou).toEqual(["a"])
  })

  it("orders requests that moved at the same moment by id, so the list never flickers", () => {
    const glance = agentsGlance(
      [session("y", 10, "needs-you"), session("x", 10, "needs-you")],
      [],
      ongoing,
    )
    expect(glance.needsYou).toEqual(["x", "y"])
  })
})

describe("sameGlance", () => {
  it("tells a glance that lists the same rows from one that moved a row", () => {
    const sessions = [session("a", 30, "needs-you"), session("b", 20, "running")]
    expect(
      sameGlance(agentsGlance(sessions, [], all), agentsGlance([...sessions], [], all)),
    ).toBe(true)
    expect(
      sameGlance(
        agentsGlance(sessions, [], all),
        agentsGlance([session("a", 30, "running"), session("b", 20, "running")], [], all),
      ),
    ).toBe(false)
  })

  it("tells a change of what is kept out alone", () => {
    const one = agentsGlance([session("a", 1)], [], ongoing)
    const two = agentsGlance([session("a", 1), session("b", 2)], [], ongoing)
    expect(sameGlance(one, two)).toBe(false)
  })
})

describe("glanceCounts", () => {
  const line = (glance: Parameters<typeof glanceCounts>[0]) =>
    glanceCounts(glance)
      .map((count) => count.label)
      .join(" · ")

  it("says only what there is, with the person's count agreeing in number", () => {
    expect(
      line(
        agentsGlance(
          [
            session("a", 1, "needs-you"),
            session("b", 2, "running"),
            session("c", 3, "running"),
          ],
          [],
          ongoing,
        ),
      ),
    ).toBe("1 needs you · 2 working")
    expect(
      line(
        agentsGlance(
          [session("a", 1, "needs-you"), session("b", 2, "needs-you")],
          [],
          ongoing,
        ),
      ),
    ).toBe("2 need you")
    expect(
      line(agentsGlance([session("a", 1, "idle", true), session("b", 2)], [], all)),
    ).toBe("1 finished · 1 earlier")
    expect(glanceCounts(agentsGlance([session("a", 1)], [], ongoing))).toEqual([])
  })

  it("keeps every count with a group shown alone, so any other can be chosen from the same line", () => {
    const sessions = [
      session("a", 1, "needs-you"),
      session("b", 2, "running"),
      session("c", 3, "idle", true),
    ]
    expect(line(agentsGlance(sessions, [], { ...all, group: "working" }))).toBe(
      "1 needs you · 1 working · 1 finished",
    )
  })

  it("keeps the group shown alone in the line at nought, so it can be let go where it was chosen", () => {
    const counts = glanceCounts(
      agentsGlance([session("a", 1, "running")], [], { ...all, group: "finished" }),
    )
    expect(counts.map((count) => [count.group, count.label])).toEqual([
      ["working", "1 working"],
      ["finished", "0 finished"],
    ])
  })
})

describe("one group shown alone", () => {
  const sessions = [
    session("ask", 30, "needs-you"),
    session("run", 20, "running"),
    session("done", 25, "idle", true),
    session("seen", 50),
  ]

  it("groups a session by where it stands", () => {
    expect(sessions.map(groupOf)).toEqual(["needsYou", "working", "finished", "earlier"])
  })

  it("lists that group only, and counts every group", () => {
    for (const [group, ids] of [
      ["needsYou", ["ask"]],
      ["working", ["run"]],
      ["finished", ["done"]],
      ["earlier", ["seen"]],
    ] as const) {
      const glance = agentsGlance(sessions, [], { ...all, group })
      expect(readingOrder(glance)).toEqual(ids)
      expect(glance.group).toBe(group)
      expect(glance.counts).toEqual({ needsYou: 1, working: 1, finished: 1, earlier: 1 })
    }
  })

  it("says it keeps out only what the filter keeps out of that group", () => {
    // Ongoing keeps out the two idle sessions: none of them is working.
    expect(agentsGlance(sessions, [], { ...ongoing, group: "working" }).hidden).toBe(0)
    expect(agentsGlance(sessions, [], { ...ongoing, group: "finished" }).hidden).toBe(1)
    expect(agentsGlance(sessions, [], ongoing).hidden).toBe(2)
  })

  it("keeps listing the session looked at when it moves to another group, until the person moves on", () => {
    const finished = [session("run", 20, "idle", true), session("other", 10, "running")]
    const looking = agentsGlance(finished, [], {
      ...all,
      group: "working",
      looking: "run",
    })
    expect(looking.working).toEqual(["other"])
    expect(looking.finished).toEqual(["run"])
    expect(agentsGlance(finished, [], { ...all, group: "working" }).finished).toEqual([])
  })

  it("shows a held request only while the waiting are shown", () => {
    const held = [{ sessionId: "b", updatedAt: 20 }]
    const moved = [session("b", 99, "running")]
    expect(agentsGlance(moved, held, { ...all, group: "needsYou" }).needsYou).toEqual([
      "b",
    ])
    expect(readingOrder(agentsGlance(moved, held, { ...all, group: "working" }))).toEqual(
      [],
    )
  })

  it("tells a change of the group shown alone", () => {
    expect(
      sameGlance(
        agentsGlance([], [], all),
        agentsGlance([], [], { ...all, group: "earlier" }),
      ),
    ).toBe(false)
  })
})
