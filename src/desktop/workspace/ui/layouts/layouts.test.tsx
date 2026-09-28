// @vitest-environment jsdom
/**
 * The two layouts differ only in their sidebar region: each is the one shell
 * with a `SidebarRegion`, so the keyboard — every key of the shared map —
 * the switcher and the titlebar's controls are the same in both.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { ClockProvider } from "../../adapters/dom/clock"
import { chordEvent } from "../../adapters/dom/shortcuts"
import { focusPane, loadWorkspace, openBeside } from "../../adapters/store/commands"
import { layoutShape, panesOf } from "../../model/pane-layout"
import { settle, testStore } from "../../testing"
import { SessionsInSidebar } from "./sessions-in-sidebar"
import { workspaceShortcuts } from "./shortcuts"
import { ThreeColumns } from "./three-columns"

let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  class Observer {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer })
  Element.prototype.scrollTo ??= () => {}
  Element.prototype.scrollIntoView ??= () => {}
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => {
  host.remove()
})

type Store = ReturnType<typeof testStore>

/** Focuses the pane at `index` in reading order. */
const focusAt = (store: Store, index: number) => {
  const panes = store.getState().workspace.panes
  const pane = panes ? panesOf(panes)[index] : undefined
  if (pane) store.dispatch(focusPane({ pane: pane.key }))
}

/**
 * The panes a key is pressed over: a and b side by side, b focused — unless
 * the key needs another arrangement to show what it does.
 */
const arrangements: Partial<Record<string, (store: Store) => void>> = {
  focusPane2: (store) => focusAt(store, 0),
  focusNext: (store) => focusAt(store, 0),
  moveRight: (store) => focusAt(store, 0),
  // b beneath a, then c beside: four ways to go.
  focusPane3: (store) => {
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    focusAt(store, 0)
  },
  focusPane4: (store) => {
    store.dispatch(openBeside({ sessionId: "c", side: "bottom" }))
    store.dispatch(openBeside({ sessionId: "d", side: "right" }))
    focusAt(store, 0)
  },
  moveUp: (store) => store.dispatch(openBeside({ sessionId: "c", side: "bottom" })),
  moveDown: (store) => {
    store.dispatch(openBeside({ sessionId: "c", side: "bottom" }))
    focusAt(store, 1)
  },
}

/** A layout over a store with a and b open side by side, arranged for `command`. */
async function render(Layout: typeof ThreeColumns, command = "") {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  await settle()
  store.dispatch(openBeside({ sessionId: "b" }))
  arrangements[command]?.(store)
  await settle()
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1000}>
          <Layout hostKind="browser" browserSurface />
        </ClockProvider>
      </Provider>,
    ),
  )
  return { store, root }
}

/** What a key did: to the panes, the columns, the content region, and any dialog it opened. */
function outcome(store: ReturnType<typeof testStore>) {
  const state = store.getState().workspace
  return {
    panes: state.panes ? layoutShape(state.panes) : "",
    focused: state.panes?.focused,
    shown: state.panes?.columns.flatMap((column) =>
      column.panes.map((pane) =>
        pane.sessionId.startsWith("id-") ? "new" : pane.sessionId,
      ),
    ),
    sidebar: state.chrome.sidebar,
    content: state.content,
    dialog: host.querySelector('[role="dialog"]')?.getAttribute("aria-label") ?? null,
  }
}

const press = (command: string) => {
  const binding = workspaceShortcuts.find((each) => each.command === command)
  if (!binding) throw new Error(`no ${command}`)
  window.dispatchEvent(
    new KeyboardEvent("keydown", { ...chordEvent(binding.chord), bubbles: true }),
  )
}

describe("both layouts are the one shell", () => {
  for (const [name, Layout] of [
    ["three columns", ThreeColumns],
    ["sessions in the sidebar", SessionsInSidebar],
  ] as const)
    it(`opens the switcher with ⌘K and ⌘\\, and carries the same titlebar, in ${name}`, async () => {
      const store = testStore()
      await store.dispatch(loadWorkspace())
      await settle()
      const root = createRoot(host)
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ClockProvider now={() => 1000}>
              <Layout hostKind="browser" browserSurface />
            </ClockProvider>
          </Provider>,
        ),
      )
      const titlebar = [...host.querySelectorAll(".workspace-titlebar button")].map(
        (button) => button.getAttribute("aria-label")?.replace(/ \(.*\)$/, ""),
      )
      expect(titlebar.slice(0, 3)).toEqual(["Hide Sidebar", "Go Back", "Go Forward"])
      await act(async () => press("switcher"))
      expect(host.querySelector('[role="dialog"]')?.getAttribute("aria-label")).toBe(
        "Jump to",
      )
      await act(async () => press("switcher"))
      expect(host.querySelector('[role="dialog"]')).toBeNull()
      await act(async () => press("openBeside"))
      expect(host.querySelector('[role="dialog"]')?.getAttribute("aria-label")).toBe(
        "Open beside",
      )
      await act(async () => root.unmount())
    })
})

describe("Agents in the sidebar is a place to go, like a channel", () => {
  for (const [name, Layout] of [
    ["three columns", ThreeColumns],
    ["sessions in the sidebar", SessionsInSidebar],
  ] as const)
    it(`opens the overview, keeps it open when chosen again, and leaves it for a channel, in ${name}`, async () => {
      const store = testStore()
      await store.dispatch(loadWorkspace())
      await settle()
      const root = createRoot(host)
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ClockProvider now={() => 1000}>
              <Layout hostKind="browser" browserSurface />
            </ClockProvider>
          </Provider>,
        ),
      )
      const agents = () => host.querySelector<HTMLButtonElement>(".agents-overview-entry")
      const current = () =>
        [...host.querySelectorAll('.workspace-sidebar [aria-current="page"]')].map(
          (row) => row.textContent?.trim(),
        )
      const channel = () =>
        [...host.querySelectorAll<HTMLButtonElement>(".workspace-sidebar button")].find(
          (row) => row.textContent?.trim().startsWith("desktop"),
        )
      const shown = () =>
        host.querySelector("[data-workspace]")?.getAttribute("data-content")

      await act(async () => agents()?.click())
      expect(shown()).toBe("agents")
      await act(async () => agents()?.click())
      expect(shown()).toBe("agents")
      expect(current()).toEqual([expect.stringMatching(/^Agents/)])

      await act(async () => channel()?.click())
      expect(shown()).toBe("panes")
      expect(current()).not.toContainEqual(expect.stringMatching(/^Agents/))
      await act(async () => root.unmount())
    })
})

describe("every key of the shared map does the same in both layouts", () => {
  // A window wide enough that neither layout folds a column for room: what a
  // key does is then the key's, not the width's.
  const width = window.innerWidth
  beforeEach(() => {
    window.innerWidth = 2400
  })
  afterEach(() => {
    window.innerWidth = width
  })
  // Where a layout has no session list, ⌥⌘S has none to show and ⌘F jumps
  // instead of searching it: the one difference the ADR names.
  const byRegion: Partial<Record<string, { columns: unknown; sidebar: unknown }>> = {
    toggleSessionList: { columns: "list", sidebar: "nothing" },
    search: { columns: null, sidebar: "Jump to" },
  }
  const pressAll = async (command: string, Layout: typeof ThreeColumns) => {
    const { store, root } = await render(Layout, command)
    const list = () => store.getState().workspace.chrome.sessionList.open
    const listBefore = list()
    const before = outcome(store)
    await act(async () => press(command))
    await act(async () => settle())
    const after = outcome(store)
    const result = { before, after, list: list() !== listBefore ? "list" : "nothing" }
    await act(async () => root.unmount())
    return result
  }
  for (const binding of workspaceShortcuts)
    it(`answers ${binding.command} alike`, async () => {
      const columns = await pressAll(binding.command, ThreeColumns)
      const sidebar = await pressAll(binding.command, SessionsInSidebar)
      const region = byRegion[binding.command]
      if (region) {
        if (binding.command === "toggleSessionList") {
          expect(columns.list).toBe(region.columns)
          expect(sidebar.list).toBe(region.sidebar)
        } else {
          expect(columns.after.dialog).toBe(region.columns)
          expect(sidebar.after.dialog).toBe(region.sidebar)
        }
        return
      }
      expect(sidebar.after).toEqual(columns.after)
      // And it did something: every key in the map is one the window answers.
      expect(columns.after, binding.command).not.toEqual(columns.before)
    })
})

describe("⌘0 and the sidebar's Agents entry always open the overview, with nothing to turn on", () => {
  for (const [name, Layout] of [
    ["three columns", ThreeColumns],
    ["sessions in the sidebar", SessionsInSidebar],
  ] as const)
    it(`in ${name}`, async () => {
      // A fresh window: nothing stored, nothing chosen in Settings.
      const { store, root } = await render(Layout)
      const entry = host.querySelector(".workspace-sidebar .agents-overview-entry")
      expect(entry?.textContent).toContain("Agents")
      // The overview holds what waits and what runs: no separate rows for them.
      const rows = [...host.querySelectorAll(".workspace-sidebar button")].map((row) =>
        row.textContent?.trim(),
      )
      expect(rows).not.toContainEqual(expect.stringMatching(/^(Needs you|Running)/))
      expect(store.getState().workspace.content).toBe("panes")
      await act(async () => press("showOverview"))
      expect(store.getState().workspace.content).toBe("agents")
      // A place, as the entry is: asked again, it stays.
      await act(async () => press("showOverview"))
      expect(store.getState().workspace.content).toBe("agents")
      await act(async () => root.unmount())
    })
})
