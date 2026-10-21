// @vitest-environment jsdom
/**
 * The two layouts differ only in their sidebar region: each is the one shell
 * with a `SidebarRegion`, so the keyboard — every key of the shared map —
 * the switcher and the titlebar's controls are the same in both.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { isMac } from "../../../adapters/platform"
import { chordEvent } from "../../../model/keyboard"
import { ClockProvider } from "../../../../clock"
import {
  closePane,
  focusPane,
  loadWorkspace,
  movePane,
  newSession,
  openBeside,
  openWidget,
  setComposerText,
  showContent,
  toggleSidebar,
} from "../../adapters/store/commands"
import { layoutShape, panesOf } from "../../../split-panes/model/pane-layout"
import {
  controlledAnimationFrames,
  fakeSource,
  flushAnimationFrames,
  settle,
  shownBy,
  testStore,
} from "../../testing"
import { paneItemOf } from "../../model/pane-item"
import { workspaceActions } from "../../adapters/store/slice"
import { selectSession } from "../../adapters/store/selectors"
import {
  openReviewEvent,
  waitingReviewNotices,
} from "../../adapters/gateway/review-notices"
import { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } from "../../../ui/menu"
import { PaneMenuItems } from "../panes/pane-menu"
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
      column.panes.map((pane) => {
        const shown = shownBy(pane)
        return shown.startsWith("id-") ? "new" : shown
      }),
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
    new KeyboardEvent("keydown", { ...chordEvent(binding.chord, isMac), bubbles: true }),
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
      // The rows' current page waits out the overview's own frames
      // (`overview-quiet.ts`).
      for (
        let frame = 0;
        frame < 48 && current().some((row) => !row?.startsWith("Agents"));
        frame++
      )
        await act(
          async () =>
            new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
        )
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

describe("the Agents entry is not marked on the open's own paint", () => {
  it("waits two frames, then takes the current page", async () => {
    const frames = controlledAnimationFrames()
    let root: Root | undefined
    try {
      ;({ root } = await render(SessionsInSidebar))
      // Mount's own frames, one `act` each, so an update that queues the next
      // frame is visible to the following step (`flushAnimationFrames`).
      // Whatever is still queued is not the entry's contract.
      await flushAnimationFrames(frames, act)
      const entry = () => host.querySelector<HTMLButtonElement>(".agents-overview-entry")
      const stacks = () => frames.pendingStacks().join("\n")
      await act(async () => entry()?.click())
      expect(entry()?.getAttribute("data-active"), stacks()).not.toBe("true")
      expect(entry()?.getAttribute("aria-current") ?? null).toBeNull()
      await act(async () => frames.runFrame())
      expect(entry()?.getAttribute("data-active"), stacks()).not.toBe("true")
      await act(async () => frames.runFrame())
      // The kit's row writes data-active either way; "true" is the mark.
      expect(entry()?.getAttribute("data-active"), stacks()).toBe("true")
      expect(entry()?.getAttribute("aria-current")).toBe("page")
    } finally {
      try {
        if (root) await act(async () => root?.unmount())
      } finally {
        frames.restore()
      }
    }
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

describe("a widget over the panes keeps the sidebar's choice, and ⌘W closes it alone", () => {
  const run = { plugin: "experiments", id: "run" }
  for (const [name, Layout] of [
    ["three columns", ThreeColumns],
    ["sessions in the sidebar", SessionsInSidebar],
  ] as const)
    it(`in ${name}`, async () => {
      const { store, root } = await render(Layout)
      const current = () =>
        [...host.querySelectorAll('.workspace-sidebar [aria-current="page"]')].map(
          (row) => row.textContent?.trim(),
        )
      const overPanes = current()
      expect(overPanes).not.toEqual([])
      await act(async () => store.dispatch(openWidget({ widget: run, place: "window" })))
      // The channel and the focused session stay chosen beside it.
      expect(current()).toEqual(overPanes)
      await act(async () => store.dispatch(showContent({ content: "agents" })))
      // The rows' current page waits out the overview's own frames
      // (`overview-quiet.ts`).
      for (
        let frame = 0;
        frame < 48 && current().some((row) => !row?.startsWith("Agents"));
        frame++
      )
        await act(
          async () =>
            new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
        )
      expect(current()).toEqual([expect.stringMatching(/^Agents/)])

      await act(async () => store.dispatch(openWidget({ widget: run, place: "window" })))
      const panes = store.getState().workspace.panes
      await act(async () => press("closePane"))
      expect(store.getState().workspace.content).toBe("panes")
      expect(store.getState().workspace.panes).toBe(panes)

      await act(async () => store.dispatch(openWidget({ widget: run, place: "window" })))
      await act(async () => press("showOverview"))
      expect(store.getState().workspace.content).toBe("agents")
      await act(async () => root.unmount())
    })
})

describe("a session's row carries its session to the panes by its pane item", () => {
  for (const [name, Layout] of [
    ["three columns", ThreeColumns],
    ["sessions in the sidebar", SessionsInSidebar],
  ] as const)
    it(`in ${name}`, async () => {
      const { root } = await render(Layout)
      const carried = [...host.querySelectorAll("[data-drag-item]")].map((row) => ({
        row: row.getAttribute("data-session-row"),
        item: paneItemOf(row.getAttribute("data-drag-item") ?? ""),
      }))
      expect(carried.length).toBeGreaterThan(0)
      for (const { row, item } of carried)
        expect(item).toEqual({ kind: "session", sessionId: row })
      await act(async () => root.unmount())
    })
})

describe("a pane's close is offered where closing it does something", () => {
  it("offers it on every pane of several, on a last one showing a session or a widget, and not on a last home", async () => {
    const { store, root } = await render(ThreeColumns)
    const offered = () =>
      [...host.querySelectorAll('.workspace-pane-header [aria-label^="Close Pane"]')].map(
        (button) => !button.hasAttribute("data-reserved"),
      )
    // And the pane's menu offers it alike: shown in a menu that is not modal,
    // as `pane-picture.test.tsx` shows it, for a modal one loads a second React.
    const menuOffers = async () => {
      const menu = document.createElement("div")
      document.body.append(menu)
      const menuRoot = createRoot(menu)
      await act(async () =>
        menuRoot.render(
          <Provider store={store}>
            <DropdownMenu open modal={false}>
              <DropdownMenuTrigger>…</DropdownMenuTrigger>
              <DropdownMenuContent>
                <PaneMenuItems
                  pane={store.getState().workspace.panes?.focused ?? 0}
                  sessionId=""
                  moves={false}
                  onChooseHeaderPicture={() => {}}
                />
              </DropdownMenuContent>
            </DropdownMenu>
          </Provider>,
        ),
      )
      const item = [...menu.ownerDocument.querySelectorAll('[role="menuitem"]')].find(
        (each) => each.textContent?.startsWith("Close Pane"),
      )
      const enabled = item ? !item.hasAttribute("data-disabled") : null
      await act(async () => menuRoot.unmount())
      menu.remove()
      return enabled
    }
    expect(offered()).toEqual([true, true])
    // jsdom runs animation frames on a timer. Stepping them keeps a busy
    // event loop from painting the closed box away before the assertion.
    const frames = controlledAnimationFrames()
    try {
      await act(async () => void store.dispatch(closePane()))
      // The closed pane's box stays through the frames a drop's copy and a
      // column title take, and it keeps the session it showed. The grid
      // paints the one pane after that.
      expect(
        [...host.querySelectorAll("article.workspace-pane")].map((pane) =>
          pane.getAttribute("aria-label"),
        ),
      ).toEqual(["Session a", "Session b"])
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
    } finally {
      frames.restore()
    }
    expect(offered()).toEqual([true])
    expect(await menuOffers()).toBe(true)
    await act(async () => void store.dispatch(newSession()))
    expect(offered()).toEqual([false])
    expect(await menuOffers()).toBe(false)
    await act(async () =>
      store.dispatch(openWidget({ widget: { plugin: "p", id: "i" }, place: "pane" })),
    )
    expect(offered()).toEqual([true])
    expect(await menuOffers()).toBe(true)
    await act(async () => root.unmount())
  })
})

describe("a source removing a session", () => {
  it("keeps that session in its box until the box goes", async () => {
    // jsdom runs animation frames on a timer. Stepping them keeps a busy
    // event loop from painting the box away before the assertion.
    const frames = controlledAnimationFrames()
    try {
      const { store, root } = await render(ThreeColumns)
      const labels = () =>
        [...host.querySelectorAll("article.workspace-pane")].map((pane) =>
          pane.getAttribute("aria-label"),
        )
      expect(labels()).toEqual(["Session a", "Session b"])
      await act(async () => {
        store.dispatch(
          workspaceActions.sessionRemoved({
            sessionId: "b",
            revision: 2,
            draftId: "fresh",
          }),
        )
      })
      expect(selectSession(store.getState(), "b")).toBeUndefined()
      expect(labels()).toEqual(["Session a", "Session b"])
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      expect(labels()).toEqual(["Session a", "Session b"])
      await act(async () => frames.runFrame())
      expect(labels()).toEqual(["Session a"])
      await act(async () => root.unmount())
    } finally {
      frames.restore()
    }
  })

  it("keeps an unsent reply in the box until the box goes", async () => {
    // The composer fills on a frame. Stepping that frame, and the ones
    // after the source removes the session, keeps the box from leaving
    // before the reply is read.
    const frames = controlledAnimationFrames()
    try {
      const { store, root } = await render(ThreeColumns)
      await act(async () => {
        store.dispatch(setComposerText({ sessionId: "b", text: "unsent reply" }))
      })
      const reply = () =>
        [...host.querySelectorAll("article.workspace-pane")].find(
          (pane) => pane.getAttribute("aria-label") === "Session b",
        )
      const textOf = (pane: Element | undefined) =>
        pane?.querySelector("textarea")?.value ?? ""
      const modelOf = (pane: Element | undefined) =>
        pane?.querySelector('[data-slot="model-picker-trigger"]')?.textContent ?? ""
      const accessOf = (pane: Element | undefined) =>
        pane?.querySelector("[data-access-mode]")?.getAttribute("data-access-mode") ?? ""
      for (let step = 0; step < 8 && textOf(reply()) !== "unsent reply"; step++)
        await act(async () => frames.runFrame())
      const model = modelOf(reply())
      const access = accessOf(reply())
      expect(textOf(reply())).toBe("unsent reply")
      expect(model.length).toBeGreaterThan(0)
      expect(access.length).toBeGreaterThan(0)
      await act(async () => {
        store.dispatch(
          workspaceActions.sessionRemoved({
            sessionId: "b",
            revision: 2,
            draftId: "fresh",
          }),
        )
      })
      expect(selectSession(store.getState(), "b")).toBeUndefined()
      expect(store.getState().workspace.composerText).not.toHaveProperty("b")
      expect(textOf(reply())).toBe("unsent reply")
      expect(modelOf(reply())).toBe(model)
      expect(accessOf(reply())).toBe(access)
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      expect(textOf(reply())).toBe("unsent reply")
      expect(modelOf(reply())).toBe(model)
      expect(accessOf(reply())).toBe(access)
      await act(async () => frames.runFrame())
      expect(reply()).toBeUndefined()
      await act(async () => root.unmount())
    } finally {
      frames.restore()
    }
  })

  it("keeps a message still on its way until the box goes", async () => {
    // The conversation fills on a frame. The message has to be on screen
    // before the source removes the session, or the box has nothing to keep.
    const frames = controlledAnimationFrames()
    try {
      const { store, root } = await render(ThreeColumns)
      const pane = () =>
        [...host.querySelectorAll("article.workspace-pane")].find(
          (item) => item.getAttribute("aria-label") === "Session b",
        )
      for (let step = 0; step < 8 && !pane()?.querySelector("textarea"); step++)
        await act(async () => frames.runFrame())
      expect(pane()?.querySelector("textarea")).not.toBeNull()
      await act(async () => {
        store.dispatch(
          workspaceActions.messageSent({
            sessionId: "b",
            initiator: "person",
            message: {
              id: "m-out",
              role: "user",
              at: 1000,
              parts: [{ kind: "text", text: "still sending" }],
              delivery: { state: "sending" },
            },
          }),
        )
      })
      await act(async () => {
        store.dispatch(
          workspaceActions.sendFailed({
            sessionId: "b",
            messageId: "m-out",
            reason: "unavailable",
          }),
        )
      })
      const bubble = () => pane()?.querySelector('[data-role="user"]')
      const failure = () => pane()?.querySelector("[data-send-failure]")
      expect(bubble()?.textContent).toContain("still sending")
      expect(failure()).not.toBeNull()
      await act(async () => {
        store.dispatch(
          workspaceActions.sessionRemoved({
            sessionId: "b",
            revision: 2,
            draftId: "fresh",
          }),
        )
      })
      expect(store.getState().workspace.outbox).not.toHaveProperty("b")
      expect(bubble()?.textContent).toContain("still sending")
      expect(failure()).not.toBeNull()
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      expect(bubble()?.textContent).toContain("still sending")
      expect(failure()).not.toBeNull()
      await act(async () => frames.runFrame())
      expect(pane()).toBeUndefined()
      await act(async () => root.unmount())
    } finally {
      frames.restore()
    }
  })

  it("keeps a failed read until the box goes", async () => {
    // The note has to be on screen before the source removes the session,
    // or the box has nothing to keep. The read never arrived, so the note
    // is the conversation.
    const frames = controlledAnimationFrames()
    try {
      const source = fakeSource()
      source.refuse("transcript", "unavailable")
      const store = testStore(source)
      await store.dispatch(loadWorkspace())
      await settle()
      store.dispatch(openBeside({ sessionId: "b" }))
      await settle()
      const root = createRoot(host)
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ClockProvider now={() => 1000}>
              <ThreeColumns hostKind="browser" browserSurface />
            </ClockProvider>
          </Provider>,
        ),
      )
      const pane = () =>
        [...host.querySelectorAll("article.workspace-pane")].find(
          (item) => item.getAttribute("aria-label") === "Session b",
        )
      const note = () => pane()?.querySelector(".workspace-transcript-note")
      for (let step = 0; step < 8 && !note(); step++)
        await act(async () => frames.runFrame())
      expect(note()?.textContent).toContain("Try Again")
      await act(async () => {
        store.dispatch(
          workspaceActions.sessionRemoved({
            sessionId: "b",
            revision: 2,
            draftId: "fresh",
          }),
        )
      })
      expect(store.getState().workspace.transcriptFailures).not.toHaveProperty("b")
      expect(note()?.textContent).toContain("Try Again")
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      expect(note()?.textContent).toContain("Try Again")
      await act(async () => frames.runFrame())
      expect(pane()).toBeUndefined()
      await act(async () => root.unmount())
    } finally {
      frames.restore()
    }
  })
})

describe("a draft's close", () => {
  it("keeps the home and its unsent text until the box goes", async () => {
    // jsdom runs animation frames on a timer. Stepping them keeps a busy
    // event loop from painting the closed box away before the assertion.
    const { store, root } = await render(ThreeColumns)
    const frames = controlledAnimationFrames()
    try {
      let draftId = ""
      await act(async () => {
        draftId = store.dispatch(newSession({ beside: "right" })) ?? ""
      })
      if (!draftId) throw new Error("no draft")
      await act(async () => {
        store.dispatch(setComposerText({ sessionId: draftId, text: "unsent line" }))
      })
      const homes = () =>
        [...host.querySelectorAll("article.workspace-pane")].map((pane) => ({
          label: pane.getAttribute("aria-label"),
          home: pane.querySelector(".workspace-pane-home") !== null,
          text: pane.querySelector("textarea")?.value ?? "",
        }))
      const showing = () => homes().find((pane) => pane.text === "unsent line")
      for (let step = 0; step < 8 && !showing(); step++)
        await act(async () => frames.runFrame())
      expect(showing()).toEqual({ label: "New session", home: true, text: "unsent line" })
      const pane = store.getState().workspace.panes
      const draftPane = pane
        ? panesOf(pane).find((item) => shownBy(item) === draftId)
        : undefined
      if (!draftPane) throw new Error("draft pane missing")
      await act(async () => void store.dispatch(closePane({ pane: draftPane.key })))
      expect(store.getState().workspace.drafts).not.toHaveProperty(draftId)
      expect(store.getState().workspace.composerText).not.toHaveProperty(draftId)
      expect(showing()).toEqual({ label: "New session", home: true, text: "unsent line" })
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      expect(showing()).toEqual({ label: "New session", home: true, text: "unsent line" })
      await act(async () => frames.runFrame())
      expect(showing()).toBeUndefined()
      await act(async () => root.unmount())
    } finally {
      frames.restore()
    }
  })
})

describe("a review notice opens its conversation", () => {
  const notices = waitingReviewNotices({ show() {}, clear() {} })
  const waitingIn = (conversationId: string) => ({
    conversationId,
    executionId: "turn",
    permissionId: "p1",
    tool: "Bash",
  })
  afterEach(() => notices.apply([]))

  async function shell() {
    const store = testStore()
    const root = createRoot(host)
    await act(async () =>
      root.render(
        <Provider store={store}>
          <ClockProvider now={() => 1000}>
            <ThreeColumns hostKind="browser" browserSurface />
          </ClockProvider>
        </Provider>,
      ),
    )
    const shown = () =>
      store
        .getState()
        .workspace.panes?.columns.flatMap((column) => column.panes.map(shownBy))
    const load = () =>
      act(async () => {
        await store.dispatch(loadWorkspace())
        await settle()
      })
    return { store, root, shown, load }
  }

  it("once the list holds it, when the notice came first", async () => {
    // The gateway can send a waiting review before the list that names its
    // conversation: the click is kept until the list has it.
    notices.apply([waitingIn("b")])
    const { store, root, shown, load } = await shell()
    const before = shown()
    await act(async () => {
      window.dispatchEvent(openReviewEvent("b"))
    })
    expect(shown()).toEqual(before)
    await load()
    expect(shown()).toContain("b")
    // Opened once: a later list change does not open it again.
    store.dispatch(openBeside({ sessionId: "a" }))
    const after = shown()
    await load()
    expect(shown()).toEqual(after)
    await act(async () => root.unmount())
  })

  it("not once no review waits there any more", async () => {
    notices.apply([waitingIn("b")])
    const { root, shown, load } = await shell()
    await act(async () => {
      window.dispatchEvent(openReviewEvent("b"))
    })
    // Answered elsewhere before the list held it: nothing is left to show.
    notices.apply([])
    await load()
    expect(shown()).not.toContain("b")
    await act(async () => root.unmount())
  })
})

describe("a spare sidebar fold paints before the boxes, and the list keeps its measure", () => {
  const narrow = { width: 500, height: 800, spare: 240 }

  it("keeps the sidebar's tree through the fold and releases the list measure after the boxes", async () => {
    const frames = controlledAnimationFrames()
    const width = window.innerWidth
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1000 })
    const store = testStore(undefined, () => narrow)
    try {
      await store.dispatch(loadWorkspace())
      await settle()
      const root = createRoot(host)
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ClockProvider now={() => 1000}>
              <ThreeColumns hostKind="browser" browserSurface />
            </ClockProvider>
          </Provider>,
        ),
      )
      const drain = async () => {
        for (let frame = 0; frame < 8 && frames.pending() > 0; frame++)
          await act(async () => frames.runFrame())
      }
      await drain()
      const sidebar = () => host.querySelector<HTMLElement>(".workspace")
      const list = () => host.querySelector<HTMLElement>("#workspace-sidebar")
      expect(sidebar()?.getAttribute("data-sidebar")).toBe("open")
      expect(list()?.hasAttribute("inert")).toBe(false)
      await act(async () => {
        store.dispatch(openBeside({ sessionId: "c" }))
      })
      expect(store.getState().workspace.chrome.sidebar.folded).toBe(true)
      expect(sidebar()?.getAttribute("data-sidebar")).toBe("open")
      expect(list()?.hasAttribute("inert")).toBe(false)
      expect(host.querySelector(".workspace-window")?.getAttribute("data-sidebar")).toBe(
        "open",
      )
      await act(async () => frames.runFrame())
      expect(sidebar()?.getAttribute("data-sidebar")).toBe("open")
      expect(list()?.hasAttribute("inert")).toBe(false)
      expect(sidebar()?.hasAttribute("data-spare-held")).toBe(true)
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(true)
      await act(async () => frames.runFrame())
      expect(sidebar()?.getAttribute("data-sidebar")).toBe("closed")
      expect(list()?.hasAttribute("inert")).toBe(false)
      expect(sidebar()?.hasAttribute("data-spare-held")).toBe(true)
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(true)
      expect(host.querySelector(".workspace-window")?.getAttribute("data-sidebar")).toBe(
        "closed",
      )
      await act(async () => frames.runFrame())
      // The boxes and the folded window land together. The list's measure
      // is still the one it had.
      expect(sidebar()?.hasAttribute("data-spare-held")).toBe(false)
      expect(list()?.hasAttribute("inert")).toBe(true)
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(true)
      await act(async () => frames.runFrame())
      // Scene text returns on the frame after the boxes. The list's measure
      // is still the one it had.
      expect(sidebar()?.hasAttribute("data-spare-held")).toBe(false)
      expect(list()?.hasAttribute("inert")).toBe(true)
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(true)
      await act(async () => frames.runFrame())
      expect(list()?.hasAttribute("inert")).toBe(true)
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(false)
      await act(async () => root.unmount())
    } finally {
      frames.restore()
      Object.defineProperty(window, "innerWidth", { configurable: true, value: width })
    }
  })

  it("keeps the list measure when a column change reverses after the boxes paint", async () => {
    const frames = controlledAnimationFrames()
    const width = window.innerWidth
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1000 })
    const store = testStore(undefined, () => narrow)
    try {
      await store.dispatch(loadWorkspace())
      await settle()
      const root = createRoot(host)
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ClockProvider now={() => 1000}>
              <ThreeColumns hostKind="browser" browserSurface />
            </ClockProvider>
          </Provider>,
        ),
      )
      for (let frame = 0; frame < 8 && frames.pending() > 0; frame++)
        await act(async () => frames.runFrame())
      const sidebar = () => host.querySelector<HTMLElement>(".workspace")
      await act(async () => {
        store.dispatch(openBeside({ sessionId: "c" }))
      })
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      expect(sidebar()?.hasAttribute("data-spare-held")).toBe(false)
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(true)
      expect(sidebar()?.getAttribute("data-sidebar")).toBe("closed")
      await act(async () => {
        store.dispatch(closePane())
      })
      // The next frame is the one that used to drop the hold, because the
      // list's painted count was still the original one.
      await act(async () => frames.runFrame())
      expect(sidebar()?.getAttribute("data-sidebar")).toBe("closed")
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(true)
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      await act(async () => frames.runFrame())
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(true)
      await act(async () => frames.runFrame())
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(false)
      await act(async () => root.unmount())
    } finally {
      frames.restore()
      Object.defineProperty(window, "innerWidth", { configurable: true, value: width })
    }
  })

  it("drops a list hold when the shape returns before the next frame", async () => {
    const frames = controlledAnimationFrames()
    const width = window.innerWidth
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1000 })
    const store = testStore(undefined, () => narrow)
    try {
      await store.dispatch(loadWorkspace())
      await settle()
      const root = createRoot(host)
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ClockProvider now={() => 1000}>
              <ThreeColumns hostKind="browser" browserSurface />
            </ClockProvider>
          </Provider>,
        ),
      )
      const drain = async () => {
        for (let frame = 0; frame < 8 && frames.pending() > 0; frame++)
          await act(async () => frames.runFrame())
      }
      await drain()
      await act(async () => {
        store.dispatch(openBeside({ sessionId: "c" }))
      })
      await drain()
      const sidebar = () => host.querySelector<HTMLElement>(".workspace")
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(false)
      const panes = store.getState().workspace.panes
      if (!panes) throw new Error("no panes")
      const keys = panesOf(panes).map((pane) => pane.key)
      const [first, second] = keys
      if (first === undefined || second === undefined) throw new Error("one pane")
      const shape = layoutShape(panes)
      // The swap queues the hold. The swap back lands before that frame.
      await act(async () => {
        store.dispatch(movePane({ pane: first, target: second, zone: "center" }))
      })
      await act(async () => {
        store.dispatch(movePane({ pane: first, target: second, zone: "center" }))
      })
      const restored = store.getState().workspace.panes
      if (!restored) throw new Error("no panes")
      expect(layoutShape(restored)).toBe(shape)
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(false)
      await act(async () => frames.runFrame())
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(false)
      await drain()
      expect(sidebar()?.hasAttribute("data-list-held")).toBe(false)
      await act(async () => root.unmount())
    } finally {
      frames.restore()
      Object.defineProperty(window, "innerWidth", { configurable: true, value: width })
    }
  })

  it("folds a sidebar the person closes before paint", async () => {
    const frames = controlledAnimationFrames()
    const store = testStore()
    try {
      await store.dispatch(loadWorkspace())
      await settle()
      const root = createRoot(host)
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ClockProvider now={() => 1000}>
              <ThreeColumns hostKind="browser" browserSurface />
            </ClockProvider>
          </Provider>,
        ),
      )
      for (let frame = 0; frame < 8 && frames.pending() > 0; frame++)
        await act(async () => frames.runFrame())
      await act(async () => {
        store.dispatch(toggleSidebar())
      })
      expect(host.querySelector(".workspace")?.getAttribute("data-sidebar")).toBe(
        "closed",
      )
      expect(frames.pending()).toBe(0)
      await act(async () => root.unmount())
    } finally {
      frames.restore()
    }
  })
})
