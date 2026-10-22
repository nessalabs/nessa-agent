import { expect, it } from "vitest"
import { focusedPane, panesOf } from "../../../split-panes/model/pane-layout"
import { paneItemKey, sessionItem } from "../../model/pane-item"
import { itemsOf, layoutOf } from "../../model/window-tabs"
import { roomyGrid, shownBy, testIndex } from "../../testing"
import {
  initialWorkspace,
  paneIds,
  shownIds,
  type WorkspaceState,
} from "../workspace-state"
import {
  closePane,
  closeTab,
  createDraft,
  dropSession,
  openBeside,
  openSession,
  splitOffTab,
} from "./panes"
import { switchTab } from "./session-tabs"
import { indexLoaded, sessionRemoved } from "./updates"

const loaded = () =>
  indexLoaded(initialWorkspace, { index: testIndex(), draftId: "draft", read: "r" })

/** Each tab's sessions, in strip order, the active tab marked with `*`. */
const strip = (state: WorkspaceState) =>
  state.tabs.tabs.map(
    (tab) =>
      (tab.id === state.tabs.activeTab ? "*" : "") +
      itemsOf(layoutOf(state.tabs, state.panes, tab))
        .map((item) => item.replace(/^s:/, ""))
        .join("+"),
  )

const key = (id: string) => paneItemKey(sessionItem(id))
const tabOf = (state: WorkspaceState, id: string) =>
  state.tabs.tabs.find((tab) =>
    itemsOf(layoutOf(state.tabs, state.panes, tab)).includes(key(id)),
  )!.id

/** a and b merged in one tab. */
const merged = () => openBeside(loaded(), { sessionId: "b", room: roomyGrid })

it("opens the first layout as the first tab", () => {
  expect(strip(loaded())).toEqual(["*a"])
})

it("opens a chat in no tab alone in a new tab, never merged into the one in front", () => {
  const next = openSession(merged(), { sessionId: "c" })
  expect(strip(next)).toEqual(["a+b", "*c"])
  expect(paneIds(next.panes)).toEqual(["c"])
  // Hidden tabs are not on screen: not read, not counted as shown.
  expect(shownIds(next).has("a")).toBe(false)
})

it("opens a new session alone in a new tab, and Split Right merges it on purpose", () => {
  const two = merged()
  const fresh = createDraft(two, { draftId: "new" })
  expect(strip(fresh)).toEqual(["a+b", "*new"])
  const split = createDraft(fresh, {
    draftId: "beside",
    beside: "right",
    room: roomyGrid,
  })
  expect(strip(split)).toEqual(["a+b", "*new+beside"])
})

it("switching tabs restores a merged tab's split exactly as it was left", () => {
  const two = merged()
  const left = two.panes
  const away = openSession(two, { sessionId: "c" })
  const back = switchTab(away, { tabId: tabOf(away, "a") })
  expect(back.panes).toBe(left)
  expect(strip(back)).toEqual(["*a+b", "c"])
})

it("opening a chat that has a tab shows that tab and focuses its pane", () => {
  const away = openSession(merged(), { sessionId: "c" })
  const next = openSession(away, { sessionId: "a" })
  expect(strip(next)).toEqual(["*a+b", "c"])
  expect(shownBy(focusedPane(next.panes!))).toBe("a")
})

it("a drop on a pane's side merges the chat out of its own tab", () => {
  const away = openSession(loaded(), { sessionId: "c" })
  const back = switchTab(away, { tabId: tabOf(away, "a") })
  const dropped = dropSession(back, {
    sessionId: "c",
    target: back.panes!.focused,
    zone: "right",
    room: roomyGrid,
  })
  expect(strip(dropped)).toEqual(["*a+c"])
})

it("dragging a chat out of a merged tab makes it a tab of its own", () => {
  const next = splitOffTab(merged(), { sessionId: "b" })
  expect(strip(next)).toEqual(["*a", "b"])
  expect(splitOffTab(next, { sessionId: "b" })).toBe(next)
})

it("closing a pane in a merged tab keeps its chat open as a tab of its own", () => {
  const two = merged()
  const pane = panesOf(two.panes!).find((each) => shownBy(each) === "b")!
  const next = closePane(two, { pane: pane.key, draftId: "draft" })
  expect(strip(next)).toEqual(["*a", "b"])
})

it("a tab's close button closes its chat: out of a group, or the whole tab", () => {
  const two = openSession(merged(), { sessionId: "c" })
  const out = closeTab(two, { sessionId: "b", draftId: "draft" })
  expect(strip(out)).toEqual(["a", "*c"])
  const gone = closeTab(out, { sessionId: "c", draftId: "draft" })
  expect(strip(gone)).toEqual(["*a"])
  expect(closeTab(gone, { sessionId: "missing", draftId: "draft" })).toBe(gone)
})

it("closing the last chat leaves one new session's home, and an unsent draft goes with its tab", () => {
  const fresh = createDraft(loaded(), { draftId: "new" })
  expect(fresh.drafts).toHaveProperty("new")
  const closed = closeTab(fresh, { sessionId: "new", draftId: "unused" })
  expect(closed.drafts).not.toHaveProperty("new")
  const last = closeTab(closed, { sessionId: "a", draftId: "home" })
  expect(strip(last)).toEqual(["*home"])
  expect(panesOf(last.panes!)).toHaveLength(1)
})

it("a draft stays while its tab is open, even hidden", () => {
  const fresh = createDraft(loaded(), { draftId: "new" })
  const away = openSession(fresh, { sessionId: "c" })
  expect(away.drafts).toHaveProperty("new")
  expect(shownIds(away).has("new")).toBe(false)
})

it("a session deleted leaves the tab it was in", () => {
  const away = openSession(merged(), { sessionId: "c" })
  const gone = sessionRemoved(away, { sessionId: "b", revision: 2, draftId: "unused" })
  expect(strip(gone)).toEqual(["a", "*c"])
})
