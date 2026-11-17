import { describe, expect, it } from "vitest"
import { makeDesktopStore } from "../../../store"
import { createQuestionFlights } from "../../application/question-flights"
import type { WindowChange } from "../../model/window-change"
import { fakeSource, keptFilter, roomyGrid, settle } from "../../testing"
import { arrangementShown, loadWorkspace, openBeside, panelShown } from "./commands"
import { carryOutWindowChange } from "./window-commands"

function storeAt() {
  return makeDesktopStore({
    workspace: fakeSource(),
    now: () => 1000,
    newId: (() => {
      let next = 0
      return () => `id-${++next}`
    })(),
    measure: () => roomyGrid,
    overviewFilter: keptFilter(),
    questionFlights: createQuestionFlights(),
  })
}

/** A timer the test runs by hand. */
function timer() {
  const due: (() => void)[] = []
  return {
    after: (_ms: number, run: () => void) => {
      due.push(run)
      return () => {
        const at = due.indexOf(run)
        if (at >= 0) due.splice(at, 1)
      }
    },
    elapse: () => due.splice(0).forEach((run) => run()),
  }
}

async function loaded() {
  const store = storeAt()
  await store.dispatch(loadWorkspace())
  await settle()
  store.dispatch(arrangementShown({ arrangement: "columns" }))
  return store
}

describe("a change asked of the window", () => {
  it("is carried out as a click would, and says when it was so already", async () => {
    const store = await loaded()
    const { after } = timer()
    const run = (change: WindowChange) => carryOutWindowChange(store, change, after)
    expect(await run({ kind: "sidebar", open: false })).toEqual({ kind: "applied" })
    expect(store.getState().workspace.chrome.sidebar.open).toBe(false)
    expect(await run({ kind: "sidebar", open: false })).toEqual({ kind: "unchanged" })
    expect(await run({ kind: "show", view: "agents" })).toEqual({ kind: "applied" })
    expect(store.getState().workspace.content).toBe("agents")
    // Opening a session goes back to the panes, beside the focused one.
    expect(await run({ kind: "open", sessionId: "b", place: "beside" })).toEqual({
      kind: "applied",
    })
    const panes = store.getState().workspace.panes!
    expect(store.getState().workspace.content).toBe("panes")
    expect(panes.columns.flatMap((column) => column.panes)).toHaveLength(2)
    // On screen already: focused where it is, and nothing else moves.
    expect(await run({ kind: "focus", sessionId: "b" })).toEqual({ kind: "unchanged" })
    expect(await run({ kind: "close", sessionId: "b" })).toEqual({ kind: "applied" })
    expect(
      store.getState().workspace.panes!.columns.flatMap((column) => column.panes),
    ).toHaveLength(1)
  })

  it("is refused for what the window does not list, show or have", async () => {
    const store = await loaded()
    const { after } = timer()
    const run = (change: WindowChange) => carryOutWindowChange(store, change, after)
    expect(await run({ kind: "open", sessionId: "nobody", place: "here" })).toEqual({
      kind: "refused",
      code: "conversation_not_listed",
    })
    expect(await run({ kind: "focus", sessionId: "b" })).toEqual({
      kind: "refused",
      code: "not_on_screen",
    })
    expect(
      await run({ kind: "panelTab", widget: { plugin: "files", id: "recent" } }),
    ).toEqual({ kind: "refused", code: "tab_not_found" })
    store.dispatch(arrangementShown({ arrangement: "sidebar" }))
    expect(await run({ kind: "sessionList", open: true })).toEqual({
      kind: "refused",
      code: "no_session_list",
    })
  })

  it("opens the panel once its layout draws it, and says covered when it does not", async () => {
    const store = await loaded()
    const clock = timer()
    const opening = carryOutWindowChange(
      store,
      { kind: "panel", open: true },
      clock.after,
    )
    expect(store.getState().workspace.panelSet).toEqual({ count: 1, open: true })
    // The layout draws it.
    store.dispatch(panelShown({ open: true }))
    expect(await opening).toEqual({ kind: "applied" })
    expect(
      await carryOutWindowChange(store, { kind: "panel", open: true }, clock.after),
    ).toEqual({ kind: "unchanged" })
    // Asked closed, and the layout never says so: past the wait it is covered.
    const closing = carryOutWindowChange(
      store,
      { kind: "panel", open: false },
      clock.after,
    )
    clock.elapse()
    expect(await closing).toEqual({ kind: "refused", code: "covered" })
  })

  it("opens to one side only where the room allows", async () => {
    const store = await loaded()
    const { after } = timer()
    store.dispatch(openBeside({ sessionId: "b" }))
    expect(
      await carryOutWindowChange(
        store,
        { kind: "open", sessionId: "c", place: "right" },
        after,
      ),
    ).toEqual({ kind: "applied" })
  })
})
