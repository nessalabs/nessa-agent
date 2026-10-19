import { describe, expect, it } from "vitest"
import { summary, testIndex } from "../../testing"
import { initialWorkspace } from "../workspace-state"
import { deleteAsked, renameDrafted, renameStarted } from "./row-editing"
import { indexLoaded, sessionRemoved, updateReceived } from "./updates"

const loaded = () =>
  indexLoaded(initialWorkspace, {
    index: testIndex(),
    draftId: "unused",
    read: "r",
  })

describe("a row's edits in place", () => {
  it("end with their session: one that comes back is not being renamed or asked about", () => {
    const editing = deleteAsked(
      renameDrafted(renameStarted(loaded(), { sessionId: "c" }), {
        sessionId: "c",
        draft: "Rain",
      }),
      { sessionId: "c" },
    )
    expect(editing.renaming).toEqual({ sessionId: "c", draft: "Rain" })
    expect(editing.deleteAsked).toBe("c")

    const removed = sessionRemoved(editing, { sessionId: "c", revision: 9, draftId: "x" })
    expect(removed.renaming).toBeNull()
    expect(removed.deleteAsked).toBeNull()

    const back = updateReceived(removed, {
      update: {
        kind: "session",
        session: summary("c", "desktop", 1, "idle", { revision: 10 }),
      },
    })
    expect(back.sessions.c).toBeDefined()
    expect(back.renaming).toBeNull()
    expect(back.deleteAsked).toBeNull()
  })

  it("about another session stay when one goes", () => {
    const editing = deleteAsked(renameStarted(loaded(), { sessionId: "a" }), {
      sessionId: "a",
    })
    const removed = sessionRemoved(editing, { sessionId: "c", revision: 9, draftId: "x" })
    expect(removed.renaming?.sessionId).toBe("a")
    expect(removed.deleteAsked).toBe("a")
  })
})
