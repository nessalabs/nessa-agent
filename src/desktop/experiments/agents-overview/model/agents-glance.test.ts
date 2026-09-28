import { describe, expect, it } from "vitest"
import type {
  SessionStatus,
  SessionSummary,
} from "../../../workspace/model/workspace-index"
import {
  agentsGlance,
  glanceLine,
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
    expect(glance.waiting).toBe(2)
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
    expect(glance.waiting).toBe(2)
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

describe("glanceLine", () => {
  it("says only what there is, with the person's count agreeing in number", () => {
    expect(
      glanceLine(
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
      glanceLine(
        agentsGlance(
          [session("a", 1, "needs-you"), session("b", 2, "needs-you")],
          [],
          ongoing,
        ),
      ),
    ).toBe("2 need you")
    expect(
      glanceLine(agentsGlance([session("a", 1, "idle", true), session("b", 2)], [], all)),
    ).toBe("1 finished · 1 earlier")
    expect(glanceLine(agentsGlance([session("a", 1)], [], ongoing))).toBe("All quiet")
  })
})
